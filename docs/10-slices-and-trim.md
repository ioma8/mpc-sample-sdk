# Chop slices and AutoTrim

Two sample-editing helpers live under `platform`: slice markers (the data behind
the Chop screen) and trim-edge detection (the data behind AutoTrim).

## Read the current slices

```rust
use mpc_sample_sdk::platform::{page, selection, slice};

let screen = page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
let Some(selected) = unsafe { selection::current(screen) } else {
    return;
};
if let Some(slices) = slice::slice_count(selected.sample()) {
    mpc_sample_sdk::platform::status::set_status(&format!(
        "{} slices, {} frames",
        slices.count, selected.frames()
    ));
}
```

`slice_count` returns `None` when the sample has no valid slice vector.

## Verify and write markers

```rust
use mpc_sample_sdk::platform::{page, selection, slice};

let screen = page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
let Some(selected) = unsafe { selection::current(screen) } else {
    return;
};
let positions = [0, 11_025, 22_050];
if slice::verify(selected.sample(), &positions, selected.frames()) {
    mpc_sample_sdk::platform::status::set_status("already marked");
    return;
}
// SAFETY: both objects came from the live Sample page in this UI callback.
if unsafe { slice::write_markers(selected.sample(), &positions, selected.frames(), screen) } {
    mpc_sample_sdk::platform::status::set_status("marked");
}
```

`write_markers` drives the app's own Reset/Insert calls, so the app owns the
slice allocation and its notifications. It returns `false` if the page has no
chop binding or the write did not take.

## Force Manual chop mode

```rust
use mpc_sample_sdk::platform::{page, slice};

let screen = page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
if unsafe { slice::set_manual(screen) } {
    mpc_sample_sdk::platform::status::set_status("Manual chop");
}
```

Check the current mode with `slice::chop_kind(screen)`; `-1` means no binding.

## AutoTrim: find the audible edges

```rust
use mpc_sample_sdk::platform::wav;
use mpc_sample_sdk::platform::trim::{self, Layout};

let path = std::path::Path::new("/data/TempProjects/Temp-Project_[ProjectData]/kick.wav");
let Some(info) = wav::Info::read(path) else {
    return;
};
let layout = Layout {
    data_offset: info.data_offset,
    channels: info.channels,
    width: info.width,
    encoding: info.encoding,
};
if let Some(edges) = trim::edges(path, layout, info.frames, 0.0) {
    mpc_sample_sdk::platform::status::set_status(&format!(
        "{}..{} of {}",
        edges.first, edges.last, info.frames
    ));
}
```

`ratio = 0.0` means the default threshold (3 % of peak, floored near -51 dBFS).
`edges` accepts PCM 16/24/32-bit and IEEE float 32/64; every read is capped to
the requested frame count, so trailing audio or RIFF chunks cannot affect the
edges. Other widths and encodings are rejected.
For audio already in memory, use `trim::memory_edges(&audio, channels, ratio)`.
