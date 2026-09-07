# midi-parser

The MIDI file reader and writer behind Maestro's converter. A small, dependency
-light crate with no audio in it at all — it turns files into events and events
back into files.

## What it handles

- **Standard MIDI Files** (RP-001, formats 0 and 1) and **MIDI Clip Files**
  (M2-116-U, the MIDI 2.0 format built on the Universal MIDI Packet), read and
  written.
- **Memory-mapped reading** — a multi-gigabyte MIDI opens instantly and is never
  loaded into RAM in one piece.
- **Track merging** into one tick-ordered event stream, so a player never has
  to reconcile tracks itself — done across threads by default (`parallel`
  feature, on [rayon](https://github.com/rayon-rs/rayon)).
- **Scanning** — duration, note and event counts, ports, channels and tempo
  changes in one pass, which is what gives the converter a progress bar before
  rendering starts.
- **Conversion** between MIDI 1.0 byte messages and MIDI 2.0 UMP packets.

## Features

| Feature | Default | Effect |
|---|---|---|
| `parallel` | yes | parse and merge tracks across threads |

Timing is reported in ticks throughout; turning ticks into seconds needs a tempo
map, which belongs to whatever is doing the playing.

```rust
let file = MidiFile::open("song.mid")?;
for event in file.merged()? {
    let event = event?;
    println!("{} ticks later: {:?}", event.delta, event.event);
}
```
