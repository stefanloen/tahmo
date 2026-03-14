use heapless::Vec;

use crate::{comms::CommsError, compute::ComputeError, measure::{MeasureResult, SectorFailError}, realtime::Deviation, types::{Config, Event, MAX_EVENTS, MAX_SECTORS, Sector}};

pub enum MeasureReqMsg {
    GetRefTime,
    MeasureSector {
        sector: Sector,
        config: Config,
        sleep_gnss: bool,
    }
}

pub enum MeasureResMsg {
    RefTimeSuccess {
        deviation: Deviation,
        date: u32,
    },
    RefTimeFail,
    SectorSuccess {
        sector_uid: u32,
        result: MeasureResult,
    },
    SectorFail {
        sector_uid: u32,
        error: SectorFailError,
    }
}

pub enum ComputeReqMsg {
    Compute {
        sector: Sector,
        config: Config,
    }
}

pub enum ComputeResMsg {
    Success {
        sector_uid: u32,
    },
    ComputeFail {
        sector_uid: u32,
        error: ComputeError
    }
}

pub enum CommReqMsg{
    Send {
        sectors: Vec<Sector, MAX_SECTORS>,
        config: Config,
        battery_mv: Option<u32>,
        temp_c: Option<f32>,
        charge_state_fraction: u8,
        events: Vec<Event, MAX_EVENTS>
    },
    GetConstellationState
}

pub enum CommResMsg{
    Success {
        sector_uids: Vec<u32, MAX_SECTORS>,
    },
    Fail {
        sector_uids: Vec<u32, MAX_SECTORS>,
        error: CommsError
    },
    ConstellationState {
        signal_bars_max: u8,
        signal_level_max: i8,
        constellation_visible: bool
    },
    ConstellationStateFail {
        error: CommsError
    }
}

pub enum MonReqMsg{
    GetBatVolt,
    GetTemp,
    ResetChargeStateMonitor,
}

pub enum MonResMsg{
    BatVoltSuccess{
        voltage: u32
    },
    BatVoltFail,
    TempSuccess{
        temp_c: f32
    },
    TempFail,
    ChargeStateFraction {
        fraction: u8
    }
}

pub enum IntReqMsg{
    BatVoltSuccess{
        voltage: u32
    },
    BatVoltFail,
    ConstellationState {
        signal_bars_max: u8,
        signal_level_max: i8,
        constellation_visible: bool
    },
    ConstellationStateFail {
        error: CommsError
    },
    GiveConfig {
        config: Config
    },
    GiveState {
        str: heapless::String<2048>
    }
}

pub enum IntResMsg{
    GetBatVolt,
    GetConstellationState,
    GetConfig,
    SetConfig {
        config: Config
    },
    GetState
}

