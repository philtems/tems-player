use anyhow::{Context, Result};
use clap::Parser;
use console::{style, Emoji, Term};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use indicatif::{ProgressBar, ProgressStyle};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use ogg::PacketReader;
use opus::Decoder as OpusDecoder;

// --- Constantes ---
static MUSIC: Emoji = Emoji("🎵 ", "");
static CHECK: Emoji = Emoji("✅ ", "");
static CROSS: Emoji = Emoji("❌ ", "");
static ERROR: Emoji = Emoji("⚠️ ", "");
static SKIP: Emoji = Emoji("⏭ ", "");
static BACK: Emoji = Emoji("⏮ ", "");
static INFO: Emoji = Emoji("ℹ️ ", "");
static HELP: Emoji = Emoji("❓ ", "");
static LIST: Emoji = Emoji("📋 ", "");
static GOTO: Emoji = Emoji("🔢 ", "");
static SEARCH: Emoji = Emoji("🔍 ", "");
static SHUFFLE: Emoji = Emoji("🔀 ", "");
static REPEAT: Emoji = Emoji("🔁 ", "");
static REPEAT_ONE: Emoji = Emoji("🔂 ", "");

#[derive(Parser, Debug)]
#[command(name = "tems-player")]
#[command(author = "Philippe TEMESI")]
#[command(version = "0.2.0")]
#[command(about = "CLI Audio Player - Support MP3, FLAC, AAC, Opus, WAV", long_about = None)]
struct Args {
    files: Vec<String>,
    #[arg(short, long)]
    directory: Vec<String>,
    #[arg(short, long)]
    m3u: Vec<String>,
}

struct FileInfo {
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
    duration_secs: f64,
    codec: String,
    file_size: u64,
    bitrate: Option<u32>,
}

#[derive(Clone, Copy, PartialEq)]
enum RepeatMode {
    Off,
    All,
    One,
}

fn cleanup_terminal() {
    print!("\x1b[?25h"); // Curseur visible
    print!("\x1b[0m");   // Reset couleurs
    print!("\r\n");      // Nouvelle ligne
    let _ = io::stdout().flush();
    
    let _ = std::process::Command::new("stty")
        .args(["sane"])
        .status();
    let _ = std::process::Command::new("stty")
        .args(["echo"])
        .status();
}

fn build_playlist(args: &Args) -> Result<Vec<PathBuf>> {
    let mut playlist = Vec::new();
    for m3u_file in &args.m3u {
        if let Ok(content) = std::fs::read_to_string(m3u_file) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                let path = PathBuf::from(line);
                if path.exists() && is_audio_file(&path) { playlist.push(path); }
            }
        }
    }
    for dir in &args.directory {
        let mut dir_playlist = Vec::new();
        for entry in WalkDir::new(dir).follow_links(true).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() && is_audio_file(path) { dir_playlist.push(path.to_path_buf()); }
        }
        dir_playlist.sort();
        playlist.extend(dir_playlist);
    }
    for file in &args.files {
        let path = PathBuf::from(file);
        if path.exists() && is_audio_file(&path) {
            playlist.push(path);
        } else if path.exists() && path.is_dir() {
            let mut dir_playlist = Vec::new();
            for entry in WalkDir::new(&path).follow_links(true).into_iter().filter_map(|e| e.ok()) {
                if entry.path().is_file() && is_audio_file(entry.path()) {
                    dir_playlist.push(entry.path().to_path_buf());
                }
            }
            dir_playlist.sort();
            playlist.extend(dir_playlist);
        }
    }
    Ok(playlist)
}

fn is_audio_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(ext.to_lowercase().as_str(), 
            "mp3" | "flac" | "aac" | "m4a" | "opus" | "ogg" | "wav" | "alac"),
        None => false,
    }
}

fn get_file_info(path: &Path) -> Result<FileInfo> {
    if path.extension().map_or(false, |ext| ext == "opus") { get_opus_info(path) } else { get_audio_info(path) }
}

fn get_opus_info(path: &Path) -> Result<FileInfo> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let buf_reader = BufReader::new(file);
    let mut packet_reader = PacketReader::new(buf_reader);
    let mut opus_decoder = OpusDecoder::new(48000, opus::Channels::Stereo)?;
    let mut pcm_buffer = vec![0i16; 11520];
    let mut total_frames = 0;
    let mut header_packets_skipped = 0;
    let mut is_decoding = false;
    while let Ok(Some(packet)) = packet_reader.read_packet() {
        if packet.data.is_empty() { continue; }
        if !is_decoding { header_packets_skipped += 1; if header_packets_skipped >= 3 { is_decoding = true; } continue; }
        if let Ok(cnt) = opus_decoder.decode(&packet.data, &mut pcm_buffer, false) { total_frames += cnt; }
    }
    let duration_secs = total_frames as f64 / 48000.0;
    let bitrate = if duration_secs > 0.0 { Some((file_size as f64 * 8.0 / duration_secs / 1000.0) as u32) } else { None };
    Ok(FileInfo { path: path.to_path_buf(), sample_rate: 48000, channels: 2, duration_secs, codec: "Opus".to_string(), file_size, bitrate })
}

fn get_audio_info(path: &Path) -> Result<FileInfo> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe().format(&Hint::new(), mss, &FormatOptions::default(), &MetadataOptions::default())?;
    let format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).context("No audio track found")?.clone();
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.unwrap_or(Default::default()).count() as u16;
    let codec = format!("{:?}", track.codec_params.codec);
    let duration_secs = if let Some(n) = track.codec_params.n_frames { n as f64 / sample_rate as f64 } else { (file_size as f64 / 24000.0).min(3600.0) };
    let bitrate = if duration_secs > 0.0 { Some((file_size as f64 * 8.0 / duration_secs / 1000.0) as u32) } else { None };
    Ok(FileInfo { path: path.to_path_buf(), sample_rate, channels, duration_secs, codec, file_size, bitrate })
}

fn load_opus_file(path: &Path) -> Result<(Vec<f32>, u32, u16)> {
    let file = File::open(path)?;
    let mut packet_reader = PacketReader::new(BufReader::new(file));
    let mut opus_decoder = OpusDecoder::new(48000, opus::Channels::Stereo)?;
    let mut pcm_buffer = vec![0i16; 11520];
    let mut all_samples = Vec::new();
    let mut header_packets_skipped = 0;
    let mut is_decoding = false;
    while let Ok(Some(packet)) = packet_reader.read_packet() {
        if packet.data.is_empty() { continue; }
        if !is_decoding { header_packets_skipped += 1; if header_packets_skipped >= 3 { is_decoding = true; } continue; }
        if let Ok(cnt) = opus_decoder.decode(&packet.data, &mut pcm_buffer, false) {
            for i in 0..cnt {
                all_samples.push((pcm_buffer[i*2] as f32 / 32768.0).clamp(-1.0, 1.0));
                all_samples.push((pcm_buffer[i*2+1] as f32 / 32768.0).clamp(-1.0, 1.0));
            }
        }
    }
    Ok((all_samples, 48000, 2))
}

fn load_audio_file(path: &Path) -> Result<(Vec<f32>, u32, u16)> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe().format(&Hint::new(), mss, &FormatOptions::default(), &MetadataOptions::default())?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).context("No audio track found")?.clone();
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.unwrap_or(Default::default()).count() as u16;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut all_samples = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id { continue; }
        if let Ok(buf) = decoder.decode(&packet) {
            all_samples.extend(convert_audio_buffer(buf, channels));
        }
    }
    Ok((all_samples, sample_rate, channels))
}

fn convert_audio_buffer(buffer: AudioBufferRef, target_channels: u16) -> Vec<f32> {
    match buffer {
        AudioBufferRef::F32(buf) => {
            let ch = buf.spec().channels.count(); let smp = buf.chan(0).len();
            let mut out = Vec::with_capacity(smp * target_channels as usize);
            for i in 0..smp {
                if ch == 1 { let s = buf.chan(0)[i]; out.push(s); if target_channels == 2 { out.push(s); } }
                else { for c in 0..ch.min(target_channels as usize) { out.push(buf.chan(c)[i]); } }
            }
            out
        }
        AudioBufferRef::S16(buf) => {
            let ch = buf.spec().channels.count(); let smp = buf.chan(0).len();
            let mut out = Vec::with_capacity(smp * target_channels as usize);
            for i in 0..smp {
                if ch == 1 { let s = buf.chan(0)[i] as f32 / 32768.0; out.push(s); if target_channels == 2 { out.push(s); } }
                else { for c in 0..ch.min(target_channels as usize) { out.push(buf.chan(c)[i] as f32 / 32768.0); } }
            }
            out
        }
        AudioBufferRef::S32(buf) => {
            let ch = buf.spec().channels.count(); let smp = buf.chan(0).len();
            let mut out = Vec::with_capacity(smp * target_channels as usize);
            for i in 0..smp {
                if ch == 1 { let s = buf.chan(0)[i] as f32 / 2147483648.0; out.push(s); if target_channels == 2 { out.push(s); } }
                else { for c in 0..ch.min(target_channels as usize) { out.push(buf.chan(c)[i] as f32 / 2147483648.0); } }
            }
            out
        }
        AudioBufferRef::U8(buf) => {
            let ch = buf.spec().channels.count(); let smp = buf.chan(0).len();
            let mut out = Vec::with_capacity(smp * target_channels as usize);
            for i in 0..smp {
                if ch == 1 { let s = (buf.chan(0)[i] as f32 - 128.0) / 128.0; out.push(s); if target_channels == 2 { out.push(s); } }
                else { for c in 0..ch.min(target_channels as usize) { out.push((buf.chan(c)[i] as f32 - 128.0) / 128.0); } }
            }
            out
        }
        _ => Vec::new()
    }
}

fn format_time(seconds: f64) -> String {
    let m = (seconds / 60.0) as u64;
    let s = (seconds % 60.0) as u64;
    format!("{:02}:{:02}", m, s)
}

fn show_help() {
    println!("{}", style("\n╔═══════════════════════════════════════════╗").cyan());
    println!("{}", style("║                Controls                   ║").cyan());
    println!("{}", style("╚═══════════════════════════════════════════╝").cyan());
    println!("  {} Space   : Play/Pause", style("•").green());
    println!("  {} n / ↓   : Next track", style("•").green());
    println!("  {} p / ↑   : Previous track", style("•").green());
    println!("  {} ← / →   : Seek -5s / +5s", style("•").green());
    println!("  {} + / =   : Increase volume", style("•").green());
    println!("  {} -       : Decrease volume", style("•").green());
    println!("  {} i       : Show current file info", style("•").green());
    println!("  {} l       : Show playlist", style("•").green());
    println!("  {} g <num> : Go to track number", style("•").green());
    println!("  {} / <text> : Search in playlist", style("•").green());
    println!("  {} s       : Toggle shuffle mode", style("•").green());
    println!("  {} r       : Toggle repeat mode", style("•").green());
    println!("  {} h       : Show this help", style("•").green());
    println!("  {} q       : Quit", style("•").green());
    println!("");
}

fn show_playlist(playlist: &[PathBuf], current_index: usize, term_width: usize) {
    println!("{}", style("\n╔═══════════════════════════════════════════╗").cyan());
    println!("{}", style("║              Playlist                     ║").cyan());
    println!("{}", style("╚═══════════════════════════════════════════╝").cyan());
    let start = if current_index > 10 { current_index - 10 } else { 0 };
    let end = (start + 20).min(playlist.len());
    for i in start..end {
        let marker = if i == current_index { "▶" } else { " " };
        let num = format!("{:3}", i + 1);
        let name = playlist[i].file_name().unwrap().to_string_lossy();
        let path_str = playlist[i].to_string_lossy();
        let max_path_len = term_width.saturating_sub(30);
        let display_path = if path_str.len() > max_path_len { format!("...{}", &path_str[path_str.len() - max_path_len + 3..]) } else { path_str.to_string() };
        println!("  {} {}. {} {}", style(marker).green().bold(), style(num).dim(), style(name).white(), style(display_path).dim());
    }
    if playlist.len() > end { println!("  ... and {} more", playlist.len() - end); }
    println!("");
}

fn show_search_results(results: &[(usize, String)], term_width: usize) {
    println!("{}", style("\n╔═══════════════════════════════════════════╗").cyan());
    println!("{}", style("║           Search Results                  ║").cyan());
    println!("{}", style("╚═══════════════════════════════════════════╝").cyan());
    for (idx, name) in results.iter().take(50) {
        let num = format!("{:3}", idx + 1);
        println!("  {}. {}", style(num).dim(), style(name).white());
    }
    if results.len() > 50 { println!("  ... and {} more results", results.len() - 50); }
    println!("");
}

fn show_file_info(info: &FileInfo, current_time: f64, term_width: usize) {
    let duration_str = format_time(info.duration_secs);
    let current_str = format_time(current_time);
    let channel_str = match info.channels { 1 => "Mono", 2 => "Stereo", _ => "Multi" };
    let size_str = if info.file_size < 1024 { format!("{} B", info.file_size) }
        else if info.file_size < 1048576 { format!("{:.1} KB", info.file_size as f64 / 1024.0) }
        else { format!("{:.1} MB", info.file_size as f64 / 1048576.0) };
    let bitrate_str = info.bitrate.map_or("N/A".to_string(), |b| format!("{} kbps", b));
    let path_str = info.path.to_string_lossy();
    let max_path_len = term_width.saturating_sub(30);
    let display_path = if path_str.len() > max_path_len { format!("...{}", &path_str[path_str.len() - max_path_len + 3..]) } else { path_str.to_string() };
    println!("{}", style("\n╔═══════════════════════════════════════════╗").cyan());
    println!("{}", style("║              File Information             ║").cyan());
    println!("{}", style("╚═══════════════════════════════════════════╝").cyan());
    println!("  {} {}", style("Name:").bold(), info.path.file_name().unwrap().to_string_lossy());
    println!("  {} {}", style("Path:").bold(), display_path);
    println!("  {} {}", style("Codec:").bold(), info.codec);
    println!("  {} {}", style("Sample Rate:").bold(), format!("{} Hz", info.sample_rate));
    println!("  {} {}", style("Channels:").bold(), channel_str);
    println!("  {} {}", style("Size:").bold(), size_str);
    println!("  {} {}", style("Bitrate:").bold(), bitrate_str);
    println!("  {} {}", style("Duration:").bold(), duration_str);
    println!("  {} {}", style("Position:").bold(), current_str);
    println!("");
}

fn search_playlist(playlist: &[PathBuf], query: &str) -> Vec<(usize, String)> {
    let query_lower = query.to_lowercase();
    let words: Vec<&str> = query_lower.split_whitespace().collect();
    let mut results = Vec::new();
    for (idx, path) in playlist.iter().enumerate() {
        let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
        let path_str = path.to_string_lossy().to_lowercase();
        let matches = if words.is_empty() { name.contains(&query_lower) || path_str.contains(&query_lower) }
            else { words.iter().all(|word| name.contains(word) || path_str.contains(word)) };
        if matches { results.push((idx, path.file_name().unwrap().to_string_lossy().to_string())); }
    }
    results
}

fn get_random_index(current: usize, playlist_len: usize) -> usize {
    let mut rng = thread_rng();
    let mut candidates: Vec<usize> = (0..playlist_len).filter(|&i| i != current).collect();
    if candidates.is_empty() { current } else { *candidates.choose(&mut rng).unwrap() }
}

fn read_input(rx: &mpsc::Receiver<console::Key>, prompt: &str, timeout: Duration) -> Option<String> {
    print!("\r\x1b[2K{}", prompt);
    io::stdout().flush().ok();
    let mut result = String::new();
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(key) => match key {
                console::Key::Char(c) if c.is_ascii_digit() => { result.push(c); print!("{}", c); io::stdout().flush().ok(); }
                console::Key::Enter => { print!("\r\x1b[2K"); io::stdout().flush().ok(); return Some(result); }
                console::Key::Char('q') => { print!("\r\x1b[2K"); io::stdout().flush().ok(); return None; }
                _ => {}
            }
            Err(_) => continue,
        }
    }
    print!("\r\x1b[2K"); io::stdout().flush().ok(); None
}

fn read_search(rx: &mpsc::Receiver<console::Key>) -> Option<String> {
    print!("\r\x1b[2K{} Search: ", SEARCH);
    io::stdout().flush().ok();
    let mut result = String::new();
    while let Ok(key) = rx.recv_timeout(Duration::from_secs(30)) {
        match key {
            console::Key::Char(c) if c.is_ascii_graphic() || c == ' ' => { result.push(c); print!("{}", c); io::stdout().flush().ok(); }
            console::Key::Char(c) if c == '\x08' || c == '\x7f' => { result.pop(); print!("\r\x1b[2K{} Search: {}", SEARCH, result); io::stdout().flush().ok(); }
            console::Key::Enter => { print!("\r\x1b[2K"); io::stdout().flush().ok(); if result.is_empty() { return None; } return Some(result); }
            console::Key::Escape => { print!("\r\x1b[2K"); io::stdout().flush().ok(); return None; }
            _ => {}
        }
    }
    print!("\r\x1b[2K"); io::stdout().flush().ok(); None
}

fn main() -> Result<()> {
    let args = Args::parse();
    let term = Term::stdout();
    let term_width = term.size().1 as usize;

    let mut playlist = build_playlist(&args)?;
    if playlist.is_empty() { cleanup_terminal(); println!("{} No audio files found!", ERROR); return Ok(()); }

    term.clear_screen()?;
    println!("{}", style("╔═══════════════════════════════════════════╗").cyan());
    println!("{}", style("║         TeMS player - multiformat         ║").cyan());
    println!("{}", style("║     2026, v0.2.0, https://www.tems.be     ║").cyan());
    println!("{}", style("╚═══════════════════════════════════════════╝").cyan());
    println!("{} Playlist: {} files", MUSIC, playlist.len());
    println!("\n{} Press {} for help", style("💡 Tip:").yellow(), style("h").green());
    println!("");

    let mut current_index = 0;
    let volume = Arc::new(Mutex::new(1.0));
    let global_stop = Arc::new(AtomicBool::new(false));
    let mut shuffle_mode = false;
    let mut repeat_mode = RepeatMode::Off;

    let (tx, rx) = mpsc::channel();
    let t_in = term.clone();
    let gs_in = global_stop.clone();
    
    let input_handle = thread::spawn(move || {
        while !gs_in.load(Ordering::Relaxed) {
            if let Ok(key) = t_in.read_key() {
                if tx.send(key).is_err() { break; }
            }
        }
    });

    while !global_stop.load(Ordering::Relaxed) {
        if current_index >= playlist.len() {
            match repeat_mode {
                RepeatMode::All => { current_index = 0; continue; }
                _ => break,
            }
        }
        
        let file = &playlist[current_index];
        let file_info = get_file_info(file)?;

        let mode_indicator = format!("{}{}", if shuffle_mode { "🔀 " } else { "" },
            match repeat_mode { RepeatMode::Off => "", RepeatMode::All => "🔁 ", RepeatMode::One => "🔂 " });
        
        println!("{} Playing: {} ({}/{}) {}", MUSIC, file.file_name().unwrap().to_string_lossy(), 
                 current_index + 1, playlist.len(), mode_indicator);

        let (samples, sample_rate, channels) = if file.extension().map_or(false, |e| e == "opus") { 
            load_opus_file(file)? } else { load_audio_file(file)? };

        let samples_len = samples.len();
        let total_secs = samples_len as f64 / (channels as f64 * sample_rate as f64);
        
        let pb = ProgressBar::new(samples_len as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {percent}% {msg}")
            .unwrap()
            .progress_chars("=> "));

        let pos = Arc::new(AtomicUsize::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let samples_arc = Arc::new(samples);

        let s_a = samples_arc.clone();
        let p_a = pos.clone();
        let pa_a = paused.clone();
        let v_a = volume.clone();
        let st_a = stop.clone();

        let play_handle = thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_output_device() { Some(d) => d, None => return };
            let config = cpal::StreamConfig { channels, sample_rate: cpal::SampleRate(sample_rate), buffer_size: cpal::BufferSize::Default };
            let p_cb = p_a.clone(); let s_cb = s_a.clone(); let pa_cb = pa_a.clone();
            let v_cb = v_a.clone(); let st_cb = st_a.clone();

            let stream = device.build_output_stream(&config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if st_cb.load(Ordering::Relaxed) { data.fill(0.0); return; }
                    let curr = p_cb.load(Ordering::Relaxed);
                    let len = s_cb.len();
                    if curr >= len { data.fill(0.0); return; }
                    let vol = *v_cb.lock().unwrap();
                    let is_paused = pa_cb.load(Ordering::Relaxed);
                    let mut i = 0;
                    let to_write = data.len().min(len - curr);
                    while i < to_write { data[i] = s_cb[curr + i] * vol; i += 1; }
                    if i < data.len() { data[i..].fill(0.0); }
                    if !is_paused { p_cb.fetch_add(i, Ordering::Relaxed); }
                },
                |err| eprintln!("{} Stream error: {}", ERROR, err), None).unwrap();
            stream.play().unwrap();
            while !st_a.load(Ordering::Relaxed) {
                if p_a.load(Ordering::Relaxed) >= samples_len { break; }
                thread::sleep(Duration::from_millis(20));
            }
            stream.pause().ok();
        });

        let mut track_done = false;
        let mut skip_to_next = false;
        let mut goto_triggered = false;
        let mut prev_triggered = false;
        let mut was_stopped = false;

        while !track_done && !global_stop.load(Ordering::Relaxed) {
            if pos.load(Ordering::Relaxed) >= samples_len {
                if repeat_mode == RepeatMode::One { pos.store(0, Ordering::Relaxed); continue; }
                stop.store(true, Ordering::Relaxed); track_done = true; break;
            }

            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(key) => match key {
                    console::Key::Char('q') => { 
                        global_stop.store(true, Ordering::Relaxed); 
                        stop.store(true, Ordering::Relaxed); 
                        was_stopped = true; 
                        track_done = true; 
                    }
                    console::Key::Char(' ') => { paused.store(!paused.load(Ordering::Relaxed), Ordering::Relaxed); }
                    console::Key::Char('n') | console::Key::ArrowDown => { stop.store(true, Ordering::Relaxed); skip_to_next = true; track_done = true; }
                    console::Key::Char('p') | console::Key::ArrowUp => { 
                        if current_index > 0 {
                            if shuffle_mode { current_index = get_random_index(current_index, playlist.len()); } else { current_index -= 1; }
                            prev_triggered = true;
                        }
                        stop.store(true, Ordering::Relaxed); track_done = true; 
                    }
                    console::Key::Char('i') => {
                        let curr = pos.load(Ordering::Relaxed);
                        let t = curr as f64 / (channels as f64 * sample_rate as f64);
                        show_file_info(&file_info, t, term_width);
                    }
                    console::Key::Char('l') => { show_playlist(&playlist, current_index, term_width); }
                    console::Key::Char('s') => { shuffle_mode = !shuffle_mode; println!("{} Shuffle: {}", SHUFFLE, if shuffle_mode { "ON" } else { "OFF" }); }
                    console::Key::Char('r') => {
                        repeat_mode = match repeat_mode { RepeatMode::Off => RepeatMode::All, RepeatMode::All => RepeatMode::One, RepeatMode::One => RepeatMode::Off };
                        let mode_str = match repeat_mode { RepeatMode::Off => "OFF", RepeatMode::All => "ALL", RepeatMode::One => "ONE" };
                        println!("{} Repeat: {}", REPEAT, mode_str);
                    }
                    console::Key::Char('/') => { if let Some(query) = read_search(&rx) { let results = search_playlist(&playlist, &query); show_search_results(&results, term_width); } }
                    console::Key::Char('g') => {
                        if let Some(num_str) = read_input(&rx, &format!("{} Track #: ", GOTO), Duration::from_secs(5)) {
                            if let Ok(num) = num_str.parse::<usize>() {
                                if num > 0 && num <= playlist.len() {
                                    current_index = num - 1; stop.store(true, Ordering::Relaxed); goto_triggered = true; track_done = true;
                                    println!("{} Going to track {}: {}", SKIP, num, playlist[current_index].file_name().unwrap().to_string_lossy());
                                } else if !num_str.is_empty() { println!("{} Invalid track number: {} (1-{})", ERROR, num, playlist.len()); }
                            }
                        }
                    }
                    console::Key::Char('+') | console::Key::Char('=') => { let mut v = volume.lock().unwrap(); *v = (*v + 0.1).min(2.0); println!("{} Volume: {:.1}", style("→").dim(), *v); }
                    console::Key::Char('-') => { let mut v = volume.lock().unwrap(); *v = (*v - 0.1).max(0.0); println!("{} Volume: {:.1}", style("→").dim(), *v); }
                    console::Key::ArrowLeft => { let c = pos.load(Ordering::Relaxed); let s = (5 * sample_rate * channels as u32) as usize; let new_pos = if c > s { c - s } else { 0 }; pos.store(new_pos, Ordering::Relaxed); println!("{} Seek -5s", BACK); }
                    console::Key::ArrowRight => { let c = pos.load(Ordering::Relaxed); let s = (5 * sample_rate * channels as u32) as usize; let new_pos = (c + s).min(samples_len); pos.store(new_pos, Ordering::Relaxed); println!("{} Seek +5s", SKIP); }
                    console::Key::Char('h') => { show_help(); }
                    _ => {}
                }
                Err(_) => {}
            }

            let curr = pos.load(Ordering::Relaxed);
            if curr < samples_len {
                pb.set_position(curr as u64);
                let t_curr = curr as f64 / (channels as f64 * sample_rate as f64);
                pb.set_message(format!("{} / {}", format_time(t_curr), format_time(total_secs)));
            }
        }

        stop.store(true, Ordering::Relaxed);
        let _ = play_handle.join();
        pb.finish_and_clear();
        
        if was_stopped { break; }
        
        if goto_triggered { goto_triggered = false; }
        else if prev_triggered { prev_triggered = false; }
        else if skip_to_next {
            if shuffle_mode { current_index = get_random_index(current_index, playlist.len()); } else { current_index += 1; }
            skip_to_next = false;
        } else if repeat_mode == RepeatMode::One { continue; }
        else if shuffle_mode { current_index = get_random_index(current_index, playlist.len()); }
        else { current_index += 1; }
    }

    cleanup_terminal();
    
    println!("{} Goodbye!", CHECK);
    Ok(())
}

