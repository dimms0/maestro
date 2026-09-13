use std::{path::PathBuf, process::exit, thread, time::Duration, time::Instant};

use maestro_core::{
    audio_params::AudioParameters, file_renderer::MaestroFileRenderer, soundfont::SoundFontList,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (midi_path, sf_path) = match (args.get(1), args.get(2)) {
        (Some(midi), Some(sf)) => (PathBuf::from(midi), PathBuf::from(sf)),
        _ => {
            eprintln!(
                "Usage: {} <midi-file> <soundfont (.sf2/.sf3/.sfz)> [output-dir]\n\n\
                 Renders <midi-file> to an audio file using the default synthesizer\n\
                 configuration. The output is written to [output-dir] (default: the\n\
                 MIDI file's directory).",
                args.first().map(String::as_str).unwrap_or("render_midi")
            );
            exit(2);
        }
    };
    let output_dir = args
        .get(3)
        .map(PathBuf::from)
        .or_else(|| midi_path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    for (label, path) in [("MIDI file", &midi_path), ("SoundFont", &sf_path)] {
        if !path.exists() {
            eprintln!("{label} not found: {}", path.display());
            exit(2);
        }
    }

    let sflist = SoundFontList::from(sf_path);
    let audio_params = AudioParameters::default();
    let renderer_config = Default::default();

    let renderer = MaestroFileRenderer::new(
        renderer_config,
        audio_params,
        sflist,
        &midi_path,
        output_dir,
    )
    .unwrap()
    .with_batch_size(0.1);

    // The statistics handle is shared: the render runs on its own thread and
    // keeps it up to date per event, this thread reads it once a second.
    let stats = renderer.get_statistics();

    let now = Instant::now();
    let render = thread::spawn(move || renderer.render());

    while !render.is_finished() {
        println!(
            "Position: {}, Voices: {}, Time: {}",
            stats.get_time(),
            stats.get_renderer().read_voice_count(),
            stats.get_renderer().get_last_render_time()
        );
        thread::sleep(Duration::from_secs(1));
    }

    render.join().unwrap().unwrap();
    let elapsed = now.elapsed().as_secs();
    println!("Render time: {}m {}s", elapsed / 60, elapsed % 60);
}
