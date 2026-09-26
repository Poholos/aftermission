# Aftermission

Post-mission review of ArduPilot dataflash (`.bin`) logs: a desktop tool
written in Rust on [egui](https://github.com/emilk/egui), reading logs
through [dflog](https://github.com/Poholos/dflog) and drawing the map
with [walkers](https://github.com/podusowski/walkers). It is for looking
at what a vehicle did after the flight, drive or dive: plotting any field
of any message against time, over the flight modes, with the values under
the cursor read out, the track on a map and the log's events in a list.

The tool is early. What works today:

- **Opening a log** by drag and drop, the file dialog (Ctrl+O) or a path on
  the command line. The file is mapped and indexed on a worker thread, so
  the window stays live; the last eight logs are kept under File > Open
  recent.
- **The side panel** lists every message type in the log with its record
  count, and unfolds each to its fields, labeled with the units the log's
  own `UNIT`, `MULT` and `FMTU` records give them (`Roll (deg)`,
  `GyrX (rad/s)`). A type logged in several instances, such as `IMU` on a
  board with three, unfolds to `IMU[0]`, `IMU[1]` and so on, each with its
  own fields. A filter box narrows the list by type or field name.
- **The plot** draws any number of fields at once. Each series sits on the
  left or the right axis, chosen from its chip above the plot; the right
  axis is scaled to the series on it and labeled in their values. Drag or
  scroll to pan, Ctrl+scroll or pinch to zoom, right-drag a box to zoom
  into it, double-click to see the whole flight again. Long series are
  thinned to the pixels on screen, keeping every spike. A legend names the
  series, and the readout under the plot gives every series' value at the
  cursor.
- **Time** reads in seconds since boot, or in UTC through the first GPS fix
  (View > Time axis). Fields are converted to their unit where the log
  says how, so `TimeUS` plots in seconds.
- **Flight modes** from the `MODE` records show as colored bands behind
  the plot, named per vehicle (Copter, Plane, Rover, Sub, AntennaTracker
  and Blimp mode tables), the vehicle read from the firmware banner.
- **The map** draws the vehicle's track from the EKF's `POS` records, or
  from `GPS` when a log has none, over OpenStreetMap tiles, with the
  plot's cursor marked on it. Hovering the track reads a point's time and
  altitude; a click seeks the plot to it. The tiles come from
  `tile.openstreetmap.org` under its [usage
  policy](https://operations.osmfoundation.org/policies/tiles/), cached on
  disk next to the settings; View > Map tiles turns them off, and the
  track then draws on a plain background with nothing downloaded.
- **Events**: the `MSG` texts, the `ERR` faults and `EV` events named per
  ArduPilot's tables, the mode changes and the parameter changes after
  boot, in one list in time order with a toggle per kind and a filter;
  parameter changes start hidden, since a mission upload can log dozens.
  A click on a row seeks the plot and the map to its time; the row the
  cursor has passed is highlighted.
- **Parameters**: a tab beside the events with every `PARM` name, its
  last value, its default where the log records one, and how often it
  changed after boot. A changed parameter unfolds to its history, the boot
  value first; a click on a change seeks the plot and the map to it.
  Toggles narrow the table to parameters off their default or changed
  after boot, and a filter to a name.
- **CSV export** (File > Export) of the series on the plot: one row per
  sample time, one column per series, a cell empty where a series has no
  sample at that time, and nothing interpolated. A `time_s` column in
  seconds since boot and, when the log has a GPS clock, a `utc` column in
  ISO 8601 to the microsecond come first. Values scaled by a power of ten
  and values from float32 fields print exactly as the log stored them. The
  whole log, or only the time range in view when the plot is zoomed.
  Series hidden through the legend stay out.
- **Parameter file export** (File > Export) as a `.param` file of
  `NAME,VALUE` lines, which Mission Planner and MAVProxy load: each
  parameter's last value in the log, or its value at boot.
- **Preferences** persist: theme, time axis, mode bands, which panels are
  open and which tab the bottom one shows, map tiles, the recent files and
  the export folder.

A plot set up on one log carries over when another opens: series the new
log also has are re-read, the rest are dropped.

Planned next: Parquet export and a browser build.

## Building and running

The toolchain's minor release is set in `rust-toolchain.toml`; `rustup
update` brings its patch releases.

```bash
cargo run --release -- path/to/flight.bin
```

On Linux, eframe and the file dialog need the GTK and X11/Wayland
development libraries; the CI workflow lists the Debian packages.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

The tests build their logs with dflog's writer, so no flight data is in the
repository.

## License

Aftermission is licensed under the GNU Affero General Public License,
version 3.0 only, and is also available under a commercial license; see
[LICENSING.md](LICENSING.md). The dflog parser it is built on is a separate
project under `MIT OR Apache-2.0`.
