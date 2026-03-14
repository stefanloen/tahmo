use defmt::{error, info, Format};
use heapless::{Vec, String};

use crate::{realtime::RealTime, utils};

// CONSTANTS
pub const SECONDS_PER_DAY: u32 = 86400;

// Control
pub const MAX_SECTORS: usize = 96;

// NMEA processing
pub const NMEA_MAX_LINES: usize = 64;
pub const NMEA_LINE_LEN: usize = 128;

pub type Nmealine = String<NMEA_LINE_LEN>;
pub type NmeaBurst = Vec<Nmealine,NMEA_MAX_LINES>;

// Storage
pub const FLASH_SIZE: usize = 4 * 1024 * 1024; // 2MB
pub const BLOCK_SIZE: usize = 4096; // 4KB
pub const NUM_CONTAINERS: usize = 51;
pub const NUM_CONTAINER_BLOCKS: usize = 15;
pub const CONTAINER_SIZE: usize = NUM_CONTAINER_BLOCKS * BLOCK_SIZE; // 60KB
pub const USABLE_SIZE: usize = NUM_CONTAINERS * CONTAINER_SIZE; // ~3MB
pub const START_ADDRESS: u32 = 0x100000 - CONTAINER_SIZE as u32; // Start at 1MB offset (flash-relative)

// Storage is divided as
// 0-49: bins (50 containers)
pub const NUM_BINS: usize = 50;
pub const BINS_CONTAINER_START: usize = 0;
// 50: measurements (1 container)
pub const NUM_MEASUREMENTS: usize = NUM_CONTAINER_BLOCKS;
pub const MEASUREMENTS_CONTAINER_START: usize = 50;
// 51: other measurements (1 container)
pub const SECTOR_CONTAINER_START: usize = 51; // 1 block
pub const SECTOR_BLOCK_START: usize = 0;
pub const CONFIG_CONTAINER_START: usize = 51; // 1 block
pub const CONFIG_BLOCK_START: usize = 1;
pub const EVENTLOG_CONTAINER_START: usize = 51; // 1 block
pub const EVENTLOG_BLOCK_START: usize = 2;

pub const BURST_SIZE: usize = 64; // Samples per burst
pub const BIN_BURST_SIZE: usize = 240;  // Burst per bin 

pub const MEASUREMENT_STORAGE_SIZE: usize = BLOCK_SIZE; // 4KB

pub const WARMUP_TIME: u32 = 30;

pub const MAX_MIDPOINTS: usize = 128;

pub type Sample = u32;
pub type Burst = Vec<Sample, BURST_SIZE>;
pub type Bin = Vec<Burst, BIN_BURST_SIZE>;

pub const EVENTS_HEADER_SIZE: usize = 4 + 4;
pub const MAX_EVENTS: usize = 32;
pub const EVENT_SIZE: usize = 9;
pub const EVENTLOG_SIZE: usize = EVENTS_HEADER_SIZE + (MAX_EVENTS*EVENT_SIZE);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    Empty = 0,
    Boot = 1
}

impl From<u8> for EventType {
    fn from(event_type: u8) -> Self {
        match event_type {
            1 => EventType::Boot,
            _ => EventType::Empty
        }
    }
}

pub trait EventCoder {
    fn to_bytes(&self) -> [u8; EVENT_SIZE];
    fn from_bytes(bytes: &[u8; EVENT_SIZE]) -> Option<Self> where Self: Sized;
}

#[derive(Debug, Clone)]
pub struct BootEvent {
    pub time: u32,
    pub date: u32,
}

const _: () = assert!(core::mem::size_of::<BootEvent>() <= EVENT_SIZE - 1);

impl BootEvent {
    pub fn new(time: u32, date: u32) -> Self {
        Self { time, date }
    }
}

impl EventCoder for BootEvent {
    fn to_bytes(&self) -> [u8; EVENT_SIZE] {
        let mut buf = [0u8; EVENT_SIZE];
        buf[0] = EventType::Boot as u8;
        buf[1..5].copy_from_slice(&self.time.to_le_bytes());
        buf[5..9].copy_from_slice(&self.date.to_le_bytes());
        buf
    }

    fn from_bytes(data: &[u8; EVENT_SIZE]) -> Option<Self> {
        if data[0] != 1 { return None; }
        Some(Self {
            time: u32::from_le_bytes(data[1..5].try_into().ok()?),
            date: u32::from_le_bytes(data[5..9].try_into().ok()?)
        })
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Boot(BootEvent),
}

impl From<BootEvent> for Event {
    fn from(e: BootEvent) -> Self {
        Event::Boot(e)
    }
}

impl Event {
    pub fn type_str(&self) -> &'static str {
        match self {
            Event::Boot(_) => "BOOT",
        }
    }

    pub fn to_bytes(&self) -> [u8; EVENT_SIZE] {
        match self {
            Event::Boot(e) => e.to_bytes(),
        }
    }

    pub fn from_bytes(data: &[u8; EVENT_SIZE]) -> Option<Self> {
        match EventType::from(data[0]) {
            EventType::Boot => BootEvent::from_bytes(data).map(Event::Boot),
            EventType::Empty => None,
        }
    }
}

pub struct EventLog {
    pub events: Vec<Event, MAX_EVENTS>,
    changed: bool,
}

impl EventLog {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            changed: false,
        }
    }

    pub fn set_changed(&mut self, changed: bool) {
        self.changed = changed;
    }

    pub fn has_changed(&self) -> bool {
        self.changed
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.changed = true;
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn push<T: Into<Event>>(&mut self, event: T) {
        let event_enum = event.into();
        if self.events.is_full() {
            self.events.remove(0);
        }
        self.events.push(event_enum).expect("Should fit");
        self.changed = true;
    }

    pub fn to_bytes(&self) -> [u8; EVENTLOG_SIZE] {
        let mut data = [0u8; EVENTLOG_SIZE];
        let num_events = self.events.len() as u32;
        data[0..4].copy_from_slice("ELOG".as_bytes());
        data[4..8].copy_from_slice(&num_events.to_le_bytes());

        for (i, event) in self.events.iter().enumerate() {
            let offset = EVENTS_HEADER_SIZE + (i * EVENT_SIZE);
            data[offset..offset + EVENT_SIZE].copy_from_slice(&event.to_bytes());
        }
        data
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < EVENTLOG_SIZE || &data[0..4] != b"ELOG" {
            return None;
        }

        let mut log = Self { events: Vec::new(), changed: false };
        let num_events = core::cmp::min(u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize, MAX_EVENTS);
        for i in 0..num_events {
            let offset = EVENTS_HEADER_SIZE + (i as usize * EVENT_SIZE);
            let event_data: &[u8; EVENT_SIZE] = data[offset..offset + EVENT_SIZE].try_into().ok()?;
            
            if let Some(event) = Event::from_bytes(event_data) {
                log.events.push(event).expect("Should fit");
            }
        }
        Some(log)
    }
}

pub const MID_TIMES_HEADER: usize = 4;
pub const MID_TIMES_SIZE: usize = MAX_MIDPOINTS * 4;
pub const CONFIG_HEADER_SIZE: usize = 4 ;
pub const CONFIG_SIZE: usize = CONFIG_HEADER_SIZE + 17 * 4 + MID_TIMES_SIZE + MID_TIMES_HEADER;

#[derive(Clone)]
pub struct Config {
    pub pre_min_elevation: u32,
    pub pre_max_elevation: u32,
    pub pre_min_azimuth: u32,
    pub pre_max_azimuth: u32,

    pub post_min_elevation: u32,
    pub post_max_elevation: u32,
    pub post_min_azimuth: u32,
    pub post_max_azimuth: u32,

    pub min_relative_height: f32,
    pub max_relative_height: f32,
    pub relative_height_step_size: f32,

    pub qc_min_elevation_range: u32,
    pub qc_iqr_size: f32,
    pub qc_min_peak_to_peak: f32,

    pub bins_per_sector: u32,
    pub seconds_per_bin: u32,

    pub measure_timeout: u32,

    pub sector_mid_times: Vec<u32, MAX_MIDPOINTS>,
}

impl Config {
    pub fn get_mid_times_as_str(&self) -> String<{MAX_MIDPOINTS * 12}> {
        let mut mid_times_str = String::<{MAX_MIDPOINTS * 12}>::new();
        for (idx, &midpoint) in self.sector_mid_times.iter().enumerate() {
            let time_str = utils::seconds_to_time_str(midpoint);
            mid_times_str.push_str(&time_str).unwrap();
            if idx < self.sector_mid_times.len() - 1 {
                mid_times_str.push_str(", ").unwrap();
            }
        }
        mid_times_str
    }

    pub fn get_measure_duration(&self) -> u32 {
        return self.bins_per_sector*self.seconds_per_bin;
    }

    pub fn to_bytes(&self) -> [u8; CONFIG_SIZE] {
        let mut data = [0u8; CONFIG_SIZE];
        let mut offset = 0;
        data[0..4].copy_from_slice("CONF".as_bytes());
        offset += 4;

        let mut push_4bytes = |bytes: [u8; 4]| {
            data[offset..offset + 4].copy_from_slice(&bytes);
            offset += 4;
        };

        push_4bytes(self.pre_min_elevation.to_le_bytes());
        push_4bytes(self.pre_max_elevation.to_le_bytes());
        push_4bytes(self.pre_min_azimuth.to_le_bytes());
        push_4bytes(self.pre_max_azimuth.to_le_bytes());

        push_4bytes(self.post_min_elevation.to_le_bytes());
        push_4bytes(self.post_max_elevation.to_le_bytes());
        push_4bytes(self.post_min_azimuth.to_le_bytes());
        push_4bytes(self.post_max_azimuth.to_le_bytes());

        push_4bytes(self.min_relative_height.to_le_bytes());
        push_4bytes(self.max_relative_height.to_le_bytes());
        push_4bytes(self.relative_height_step_size.to_le_bytes());

        push_4bytes(self.qc_min_elevation_range.to_le_bytes());
        push_4bytes(self.qc_iqr_size.to_le_bytes());
        push_4bytes(self.qc_min_peak_to_peak.to_le_bytes());

        push_4bytes(self.bins_per_sector.to_le_bytes());
        push_4bytes(self.seconds_per_bin.to_le_bytes());
        push_4bytes(self.measure_timeout.to_le_bytes());

        push_4bytes((self.sector_mid_times.len() as u32).to_le_bytes());
        for &time in self.sector_mid_times.iter() {
            push_4bytes(time.to_le_bytes());
        }

        data
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        // Basic length check to prevent panics during slicing
        if data.len() < CONFIG_SIZE || &data[0..4] != b"CONF" {
            return None;
        }

        let mut offset = 4;

        let mut next_4bytes = || {
            let val = data[offset..offset + 4].try_into().unwrap();
            offset += 4;
            val
        };

        Some(Self {
            pre_min_elevation: u32::from_le_bytes(next_4bytes()),
            pre_max_elevation: u32::from_le_bytes(next_4bytes()),
            pre_min_azimuth: u32::from_le_bytes(next_4bytes()),
            pre_max_azimuth: u32::from_le_bytes(next_4bytes()),

            post_min_elevation: u32::from_le_bytes(next_4bytes()),
            post_max_elevation: u32::from_le_bytes(next_4bytes()),
            post_min_azimuth: u32::from_le_bytes(next_4bytes()),
            post_max_azimuth: u32::from_le_bytes(next_4bytes()),

            min_relative_height: f32::from_le_bytes(next_4bytes()),
            max_relative_height: f32::from_le_bytes(next_4bytes()),
            relative_height_step_size: f32::from_le_bytes(next_4bytes()),

            qc_min_elevation_range: u32::from_le_bytes(next_4bytes()),
            qc_iqr_size: f32::from_le_bytes(next_4bytes()),
            qc_min_peak_to_peak: f32::from_le_bytes(next_4bytes()),

            bins_per_sector: u32::from_le_bytes(next_4bytes()),
            seconds_per_bin: u32::from_le_bytes(next_4bytes()),
            measure_timeout: u32::from_le_bytes(next_4bytes()),

            sector_mid_times: {
                let len = u32::from_le_bytes(next_4bytes()) as usize;
                if len > MAX_MIDPOINTS {
                    return None;
                }
                let mut v = Vec::<u32, MAX_MIDPOINTS>::new();
                for _ in 0..len {
                    v.push(u32::from_le_bytes(next_4bytes())).ok()?;
                }
                v
            },
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut mid_times = Vec::<u32, MAX_MIDPOINTS>::new();
        mid_times.push(utils::time_str_to_seconds("00:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("01:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("02:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("03:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("04:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("05:00:00").unwrap()).unwrap();
        mid_times.push(utils::time_str_to_seconds("06:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("07:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("08:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("09:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("10:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("11:00:00").unwrap()).unwrap();
        mid_times.push(utils::time_str_to_seconds("12:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("13:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("14:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("15:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("16:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("17:00:00").unwrap()).unwrap();
        mid_times.push(utils::time_str_to_seconds("18:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("19:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("20:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("21:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("22:00:00").unwrap()).unwrap();
        // mid_times.push(utils::time_str_to_seconds("23:00:00").unwrap()).unwrap();
        
        Self {
            pre_min_elevation: 5,
            pre_max_elevation: 30,
            pre_min_azimuth: 120,
            pre_max_azimuth: 240,

            post_min_elevation: 5,
            post_max_elevation: 30,
            post_min_azimuth: 120,
            post_max_azimuth: 240,

            min_relative_height: 0.5,
            max_relative_height: 15.0,
            relative_height_step_size: 0.03,

            qc_min_elevation_range: 8,
            qc_iqr_size: 1.5,
            qc_min_peak_to_peak: 2.0,

            sector_mid_times: mid_times,

            bins_per_sector: 3,
            seconds_per_bin: 240*5,

            measure_timeout: 60,
        }
    }
}

pub const SECTOR_LIST_HEADER_SIZE: usize = 4 + 4;
pub const SECTOR_LIST_SIZE: usize = SECTOR_LIST_HEADER_SIZE + MAX_SECTORS * SECTOR_SIZE;

pub struct SectorList {
    sectors: Vec<Sector, MAX_SECTORS>,
    changed: bool,
}

impl SectorList {
    pub fn new() -> Self {
        Self {
            sectors: Vec::new(),
            changed: false,
        }
    }

    pub fn set_changed(&mut self, changed: bool) {
        self.changed = changed;
    }

    pub fn has_changed(&self) -> bool {
        self.changed
    }

    pub fn len(&self) -> usize {
        self.sectors.len()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, Sector> {
        self.sectors.iter()
    }

    pub fn clear(&mut self) {
        self.sectors.clear()
    }

    pub fn from_bytes(data: &[u8], update: bool) -> Option<Self> {
        let mut sectors = Vec::<Sector, MAX_SECTORS>::new();
        if &data[0..4] != b"SECT" {
            return None;
        }
        let num_sectors = u32::from_le_bytes(data[4..8].try_into().unwrap());
        for i in 0..num_sectors {
            let start = SECTOR_LIST_HEADER_SIZE + i as usize * SECTOR_SIZE;
            let end = start + SECTOR_SIZE;
            let mut sector = Sector::from_bytes(&data[start..end]);

            if update {
                match sector.state {
                    SectorState::AWAITING | SectorState::TO_MEASURE | SectorState::MEASURING => {
                        info!("[sect] loaded sector {} in state {:?}, deleting", sector.get_uid(), sector.state);
                        continue;
                    }
                    SectorState::COMPUTING => {
                        info!("[sect] loaded sector {} in state {:?}, setting to TO_COMPUTE", sector.get_uid(), sector.state);
                        sector.state = SectorState::TO_COMPUTE;
                    }
                    SectorState::COMMUNICATING => {
                        info!("[sect] loaded sector {} in state {:?}, setting to TO_COMMUNICATE", sector.get_uid(), sector.state);
                        sector.state = SectorState::TO_COMMUNICATE;
                    }
                    SectorState::DONE => {
                        info!("[sect] loaded sector {} in state {:?}, deleteing", sector.get_uid(), sector.state);
                        continue;
                    }
                    _ => { /* keep state as is */ }
                }
            }

            sectors.push(sector).expect("Should fit");
        }
        Some(Self {
            sectors,
            changed: false,
        })
    }

    pub fn to_bytes(&self) -> [u8; SECTOR_LIST_SIZE] {
        let mut data = [0u8; SECTOR_LIST_SIZE];
        let num_sectors = self.sectors.len() as u32;
        data[0..4].copy_from_slice("SECT".as_bytes());
        data[4..8].copy_from_slice(&num_sectors.to_le_bytes());
        for (i, sector) in self.sectors.iter().enumerate() {
            let sector_bytes = sector.to_bytes();
            let start = SECTOR_LIST_HEADER_SIZE + i * SECTOR_SIZE;
            let end = start + SECTOR_SIZE;
            data[start..end].copy_from_slice(&sector_bytes);
        }
        data
    }

    pub fn push(&mut self, sector: Sector) {
        self.sectors.push(sector).expect("Should fit");
    }

    pub fn delete_uid(&mut self, uid: u32) {
        let index = self.sectors.iter().position(|s| s.get_uid() == uid);
        if let Some(i) = index {
            self.sectors.remove(i);
        } else {
            error!("Tried to delete sector with uid {} but it was not found", uid);
        }
    }

    pub fn get_mut(&mut self, index: usize) -> &mut Sector {
        self.sectors.get_mut(index).expect("Only use after getting id directly")
    }

    pub fn get(&self, index: usize) -> &Sector {
        self.sectors.get(index).expect("Only use after getting id directly")
    }

    pub fn get_mut_uid(&mut self, uid: u32) -> Option<&mut Sector> {
        for sector in self.sectors.iter_mut() {
            if sector.get_uid() == uid {
                return Some(sector);
            }
        }
        None
    }

    pub fn get_idx_for_state(&self, state: SectorState) -> Option<usize> {
        for (i, sector) in self.sectors.iter().enumerate() {
            if sector.state == state {
                return Some(i);
            }
        }
        None
    }

    pub fn get_idxs_for_state(&self, state: SectorState) -> Vec<usize, MAX_SECTORS> {
        let mut indices = Vec::<usize, MAX_SECTORS>::new();
        for (i, sector) in self.sectors.iter().enumerate() {
            if sector.state == state {
                indices.push(i).expect("Should fit");
            }
        }
        indices
    }

    pub fn print_debug(&self) {
        info!("--- SectorList Report ---");
        for s in self.sectors.iter() {
            info!("{:?}", s); 
        }
        info!("--- End of Report ---");
    }
}

# [derive(Clone, Copy, Debug, PartialEq, Eq, Format)]
pub enum SectorState {
    AWAITING,
    TO_MEASURE,
    MEASURING,
    TO_COMPUTE,
    COMPUTING,
    TO_COMMUNICATE,
    COMMUNICATING,
    DONE
}

pub const SECTOR_SIZE: usize = 41;

#[derive(Clone, Debug, Format)]
pub struct Sector {
    uid: u32,

    midpoint_index: u32,
    measurement_index: u32,
    start_bin_index: u32,
    start_time: u32,
    start_day: u32,
    n_bins: u32,

    seconds_per_bin: u32,
    pub state: SectorState,

    lat: f32,
    lon: f32,
}

impl Sector {
    pub fn new(uid: u32, midpoint_index: u32, measurement_index: u32, start_bin_index: u32, start_time: u32, start_day: u32, n_bins: u32, seconds_per_bin: u32, state: SectorState) -> Self {
        Self {
            uid,
            midpoint_index,
            measurement_index,
            start_bin_index,
            start_time,
            start_day,
            n_bins,
            seconds_per_bin,
            state,
            lat: 0.0,
            lon: 0.0,
        }
    }

    pub fn to_bytes(&self) -> [u8; SECTOR_SIZE] {
        let mut data = [0u8; SECTOR_SIZE];
        data[0..4].copy_from_slice(&self.uid.to_le_bytes());
        data[4..8].copy_from_slice(&self.midpoint_index.to_le_bytes());
        data[8..12].copy_from_slice(&self.measurement_index.to_le_bytes());
        data[12..16].copy_from_slice(&self.start_bin_index.to_le_bytes());
        data[16..20].copy_from_slice(&self.start_time.to_le_bytes());
        data[20..24].copy_from_slice(&self.start_day.to_le_bytes());
        data[24..28].copy_from_slice(&self.n_bins.to_le_bytes());
        data[28..32].copy_from_slice(&self.seconds_per_bin.to_le_bytes());
        data[32] = match self.state {
            SectorState::AWAITING => 0,
            SectorState::TO_MEASURE => 1,
            SectorState::MEASURING => 2,
            SectorState::TO_COMPUTE => 3,
            SectorState::COMPUTING => 4,
            SectorState::TO_COMMUNICATE => 5,
            SectorState::COMMUNICATING => 6,
            SectorState::DONE => 7,
        };
        data[33..37].copy_from_slice(&self.lat.to_le_bytes());
        data[37..41].copy_from_slice(&self.lon.to_le_bytes());
        data
    }

    pub fn from_bytes(data: &[u8]) -> Self {
        let uid = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let midpoint_index = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let measurement_index = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let start_bin_index = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let start_time = u32::from_le_bytes(data[16..20].try_into().unwrap());
        let start_day = u32::from_le_bytes(data[20..24].try_into().unwrap());
        let n_bins = u32::from_le_bytes(data[24..28].try_into().unwrap());
        let seconds_per_bin = u32::from_le_bytes(data[28..32].try_into().unwrap());
        let state = match data[32] {
            0 => SectorState::AWAITING,
            1 => SectorState::TO_MEASURE,
            2 => SectorState::MEASURING,
            3 => SectorState::TO_COMPUTE,
            4 => SectorState::COMPUTING,
            5 => SectorState::TO_COMMUNICATE,
            6 => SectorState::COMMUNICATING,
            7 => SectorState::DONE,
            _ => SectorState::AWAITING,
        };
        let lat = f32::from_le_bytes(data[33..37].try_into().unwrap());
        let lon = f32::from_le_bytes(data[37..41].try_into().unwrap());
        Self {
            uid,
            midpoint_index,
            measurement_index,
            start_bin_index,
            start_time,
            start_day,
            n_bins,
            seconds_per_bin,
            state,
            lat,
            lon,
        }
    }

    pub fn update_coords(&mut self, lat: f32, lon: f32) {
        self.lat = lat;
        self.lon = lon;
    }

    pub fn get_uid(&self) -> u32 {
        self.uid
    }

    pub fn get_midpoint_index(&self) -> u32 {
        self.midpoint_index
    }

    pub fn get_measurement_index(&self) -> u32 {
        self.measurement_index
    }

    pub fn get_start_bin_index(&self) -> u32 {
        self.start_bin_index
    }

    pub fn get_start_time(&self) -> u32 {
        self.start_time
    }

    pub fn get_start_day(&self) -> u32 {
        self.start_day
    }

    pub fn get_end_time(&self) -> u32 {
        (self.start_time + self.n_bins * self.seconds_per_bin) % SECONDS_PER_DAY
    }

    pub fn get_lat(&self) -> f32 {
        self.lat
    }

    pub fn get_lon(&self) -> f32 {
        self.lon
    }

    pub fn get_bins(&self) -> Vec<u32, 128> {
        let mut bins = Vec::<u32, 128>::new();
        for i in 0..self.n_bins {
            bins.push((self.start_bin_index + i) % NUM_BINS as u32).unwrap();
        }
        bins
    }

    pub fn get_bin_for_time(&self, time: u32) -> u32 {
        let dt = if time >= self.start_time {
            time - self.start_time
        } else {
            SECONDS_PER_DAY + time - self.start_time
        };
        let bin_id = self.start_bin_index + dt / self.seconds_per_bin;

        bin_id % NUM_BINS as u32
    }

    pub fn is_before_sector(&self, time: u32, date: u32) -> bool {
        time < self.get_start_time() && date <= self.start_day
    }

    pub fn ends_next_day(&self) -> bool {
        let end_time = self.get_end_time();
        end_time < self.start_time
    }

    pub fn is_after_sector(&self, time: u32, date: u32) -> bool {
        let end_day = if self.ends_next_day() {
            self.start_day + 1
        } else {
            self.start_day
        };
        time >= self.get_end_time() && date >= end_day
    }

    pub fn is_succeeding(&self, previous: &Sector) -> bool {
        RealTime::diff_wrapping(self.start_time, previous.get_end_time()) < 30 // TODO remove magic number
    }
}

pub const MAX_MEASUREMENT_OBSERVATIONS: usize = 128;

pub const MEASUREMENT_HEADER_SIZE: usize = 32;
pub const OBSERVATION_SIZE: usize = 31;

pub const MEASUREMENT_SIZE: usize = MEASUREMENT_HEADER_SIZE + (OBSERVATION_SIZE * MAX_MEASUREMENT_OBSERVATIONS);

pub struct Measurement {
    pub uid: u32,
    pub num_seen: u32,
    pub observations: Vec<Observation, MAX_MEASUREMENT_OBSERVATIONS>,
    pub start_time: u32,
    pub end_time: u32,
    pub mean: f32,
    pub std: f32,
    pub lat: f32,
    pub lon: f32,
}

impl Measurement {
    pub fn new(uid: u32, num_seen: u32, start_time: u32, end_time: u32, mean: f32, std: f32, lat: f32, lon: f32) -> Self {
        Self {
            uid,
            num_seen,
            observations: Vec::new(),
            start_time,
            end_time,
            mean,
            std,
            lat,
            lon,
        }
    }

    pub fn from_bytes(data: &[u8; MEASUREMENT_SIZE]) -> Option<Self> {
        let start_time = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let end_time = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let num_seen = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let mean = f32::from_le_bytes([data[12], data[13], data[14], data[15]]);
        let std = f32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        let uid = f32::from_le_bytes([data[20], data[21], data[22], data[23]]) as u32;
        let lat = f32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let lon = f32::from_le_bytes([data[28], data[29], data[30], data[31]]);
        // info!("Header: {:?} -> {},{},{},{},{}", &data[0..20], start_time, end_time, num_seen, mean, std);

        let mut observations = Vec::<Observation, MAX_MEASUREMENT_OBSERVATIONS>::new();

        for i in 0..MAX_MEASUREMENT_OBSERVATIONS {
            let offset = MEASUREMENT_HEADER_SIZE + i * OBSERVATION_SIZE;
            let obs_data: &[u8; OBSERVATION_SIZE] = data[offset..offset + OBSERVATION_SIZE].try_into().unwrap();
            let observation = Observation::from_bytes(obs_data);
            if observation.used {
                observations.push(observation).ok();
            }
        }

        Some(Self {
            uid,
            num_seen,
            observations,
            start_time,
            end_time,
            mean,
            std,
            lat,
            lon,
        })
    }

    pub fn push(&mut self, obs: Observation) {
        self.observations.push(obs).ok();
    }

    pub fn to_bytes(&self) -> [u8; MEASUREMENT_SIZE] {
        let mut data = [0u8; MEASUREMENT_SIZE];
        data[0..4].copy_from_slice(&self.start_time.to_le_bytes());
        data[4..8].copy_from_slice(&self.end_time.to_le_bytes());
        data[8..12].copy_from_slice(&self.num_seen.to_le_bytes());
        data[12..16].copy_from_slice(&self.mean.to_le_bytes());
        data[16..20].copy_from_slice(&self.std.to_le_bytes());
        data[20..24].copy_from_slice(&(self.uid as f32).to_le_bytes());
        data[24..28].copy_from_slice(&self.lat.to_le_bytes());
        data[28..32].copy_from_slice(&self.lon.to_le_bytes());
        // info!("Header: {},{},{},{},{} -> {:?}", self.start_time, self.end_time, self.num_seen, self.mean, self.std, &data[0..20]);

        for (i, obs) in self.observations.iter().enumerate() {
            let obs_bytes = obs.to_bytes();
            data[MEASUREMENT_HEADER_SIZE + i * OBSERVATION_SIZE..MEASUREMENT_HEADER_SIZE + (i + 1) * OBSERVATION_SIZE].copy_from_slice(&obs_bytes);
        }

        data
    }
}

pub struct Observation {
    pub sat_id: u16,
    pub start_time: u16,
    pub end_time: u16,
    pub max_amp: f32,
    pub max_rh: f32,
    pub max_amp_2: f32,
    pub max_rh_2: f32,
    pub mean_amp: f32,
    pub num_recs: u32,
    pub used: bool,
}

impl Observation {
    pub fn from_bytes(data: &[u8; OBSERVATION_SIZE]) -> Self {
        let sat_id = u16::from_le_bytes([data[0], data[1]]);
        let start_time = u16::from_le_bytes([data[2], data[3]]);
        let end_time = u16::from_le_bytes([data[4], data[5]]);
        let max_amp = f32::from_le_bytes([data[6], data[7], data[8], data[9]]);
        let max_rh = f32::from_le_bytes([data[10], data[11], data[12], data[13]]);
        let mean_amp = f32::from_le_bytes([data[14], data[15], data[16], data[17]]);
        let num_recs = u32::from_le_bytes([data[18], data[19], data[20], data[21]]);
        let used = data[22] != 0;
        let max_amp_2 = f32::from_le_bytes([data[23], data[24], data[25], data[26]]);
        let max_rh_2 = f32::from_le_bytes([data[27], data[28], data[29], data[30]]);

        Self {
            sat_id,
            start_time,
            end_time,
            max_amp,
            max_rh,
            mean_amp,
            num_recs,
            used,
            max_amp_2,
            max_rh_2,
        }
    }

    #[inline]
    pub fn peak_to_mean(&self) -> f32 {
        if self.mean_amp > 0.0 {
            self.max_amp / self.mean_amp
        } else {
            0.0
        }
    }

    pub fn to_bytes(&self) -> [u8; OBSERVATION_SIZE] {
        let mut data = [0u8; OBSERVATION_SIZE];
        data[0..2].copy_from_slice(&self.sat_id.to_le_bytes());
        data[2..4].copy_from_slice(&self.start_time.to_le_bytes());
        data[4..6].copy_from_slice(&self.end_time.to_le_bytes());
        data[6..10].copy_from_slice(&self.max_amp.to_le_bytes());
        data[10..14].copy_from_slice(&self.max_rh.to_le_bytes());
        data[14..18].copy_from_slice(&self.mean_amp.to_le_bytes());
        data[18..22].copy_from_slice(&self.num_recs.to_le_bytes());
        data[22] = if self.used { 1 } else { 0 };
        data[23..27].copy_from_slice(&self.max_amp_2.to_le_bytes());
        data[27..31].copy_from_slice(&self.max_rh_2.to_le_bytes());
        data
    }
}