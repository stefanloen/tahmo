# realtime-gnssrefl
The repository for the water level measurement system from TAHMO.

The project contains the following parts

## control
Contains the firmware for the Pico 2. For installation ensure Rust is installed and use the following commands
```bash
$ cd control
$ rustup target add thumbv8m.main-none-eabihf
```
And flash and run the firmware with the following commands:
```bash
$ cd control
$ cargo run --release
```

Creating an uf2 file:
```bash
$ picotool uf2 convert .\target\thumbv8m.main-none-eabihf\release\control -t elf control.uf2
```


## analysis
Provides tools for analysing GNSS date (similar to what is in the firmware), and for seeing the results from a firmware dump.

## dashboard
Contains the code for the dashboard. Ensure python is installedand use the following commands:
```bash
$ cd dashboard
$ pip install flask
```
And to start the dashboard run:
```bash
$ cd dashboard
$ python main.py
```

## hardware
Contains the KiCAD PCB design files as raw project folder and as archive. Also contains some design documentation.
