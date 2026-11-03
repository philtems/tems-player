use super::{
    build_playlist, get_file_info, is_audio_file, load_audio_file, load_opus_file, play_audio,
    Args, RepeatMode,
};
use anyhow::Result;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Snapshot {
    playlist: Vec<PathBuf>,
    current: Option<usize>,
    position: f64,
    duration: f64,
    volume: f32,
    crossfade_seconds: f32,
    paused: bool,
    shuffle: bool,
    repeat: RepeatMode,
    error: Option<String>,
    metadata: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            playlist: Vec::new(),
            current: None,
            position: 0.0,
            duration: 0.0,
            volume: 1.0,
            crossfade_seconds: 0.0,
            paused: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            error: None,
            metadata: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum InsertMode {
    Replace,
    Append,
    Prepend,
}

enum Command {
    Add(Vec<PathBuf>, InsertMode),
    Remove(usize),
    RemoveMany(Vec<usize>),
    Clear,
    Move(usize, usize),
    MoveAfterCurrent(usize),
    MoveAfterCurrentMany(Vec<usize>),
    Play(usize),
    TogglePause,
    Stop,
    Next,
    Previous,
    Seek(f64),
    Volume(f32),
    Crossfade(f32),
    Shuffle,
    Repeat,
    Quit,
    GoTo(usize),
}

pub fn run(args: Args) -> Result<()> {
    std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    let playlist = build_playlist(&args)?;
    let (tx, rx) = mpsc::channel();
    let (updates_tx, updates_rx) = mpsc::channel();
    thread::spawn(move || playback_worker(playlist, rx, updates_tx));

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "TeMS-Player",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(PlayerApp::new(tx, updates_rx)))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

struct PlayerApp {
    tx: mpsc::Sender<Command>,
    updates: mpsc::Receiver<Snapshot>,
    state: Snapshot,
    selected: HashSet<usize>,
    selection_anchor: Option<usize>,
    search: String,
    about_open: bool,
}

impl PlayerApp {
    fn new(tx: mpsc::Sender<Command>, updates: mpsc::Receiver<Snapshot>) -> Self {
        Self {
            tx,
            updates,
            state: Snapshot::default(),
            selected: HashSet::new(),
            selection_anchor: None,
            search: String::new(),
            about_open: false,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    fn add_dialog(&self, mode: InsertMode, kind: AddKind) {
        let paths = match kind {
            AddKind::Files => rfd::FileDialog::new().pick_files().unwrap_or_default(),
            AddKind::Playlist => rfd::FileDialog::new()
                .add_filter("Playlists", &["m3u", "m3u8"])
                .pick_file()
                .into_iter()
                .collect(),
            AddKind::Folder => rfd::FileDialog::new()
                .pick_folder()
                .map(|path| vec![path])
                .unwrap_or_default(),
        };
        if !paths.is_empty() {
            let tx = self.tx.clone();
            thread::spawn(move || {
                let paths = match kind {
                    AddKind::Files => paths
                        .into_iter()
                        .filter(|path| path.is_file() && is_audio_file(path))
                        .collect(),
                    AddKind::Folder => build_playlist(&Args {
                        files: Vec::new(),
                        directory: paths
                            .iter()
                            .map(|path| path.to_string_lossy().to_string())
                            .collect(),
                        m3u: Vec::new(),
                        gui: true,
                    })
                    .unwrap_or_default(),
                    AddKind::Playlist => paths
                        .into_iter()
                        .flat_map(|path| {
                            build_playlist(&Args {
                                files: Vec::new(),
                                directory: Vec::new(),
                                m3u: vec![path.to_string_lossy().to_string()],
                                gui: true,
                            })
                            .unwrap_or_default()
                        })
                        .collect(),
                };
                let _ = tx.send(Command::Add(paths, mode));
            });
        }
    }
}

#[derive(Clone, Copy)]
enum AddKind {
    Files,
    Playlist,
    Folder,
}

impl eframe::App for PlayerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(snapshot) = self.updates.try_recv() {
            self.state = snapshot;
        }
        let dropped: Vec<PathBuf> = ctx
            .input(|input| input.raw.dropped_files.clone())
            .into_iter()
            .filter_map(|file| file.path)
            .collect();
        if !dropped.is_empty() {
            let tx = self.tx.clone();
            thread::spawn(move || {
                let mut audio_paths = Vec::new();
                for path in dropped {
                    if path.is_file() && is_audio_file(&path) {
                        audio_paths.push(path);
                    } else if path.is_file()
                        && path.extension().is_some_and(|extension| {
                            extension.eq_ignore_ascii_case("m3u")
                                || extension.eq_ignore_ascii_case("m3u8")
                        })
                    {
                        audio_paths.extend(
                            build_playlist(&Args {
                                files: Vec::new(),
                                directory: Vec::new(),
                                m3u: vec![path.to_string_lossy().to_string()],
                                gui: true,
                            })
                            .unwrap_or_default(),
                        );
                    } else if path.is_dir() {
                        audio_paths.extend(
                            build_playlist(&Args {
                                files: Vec::new(),
                                directory: vec![path.to_string_lossy().to_string()],
                                m3u: Vec::new(),
                                gui: true,
                            })
                            .unwrap_or_default(),
                        );
                    }
                }
                if !audio_paths.is_empty() {
                    let _ = tx.send(Command::Add(audio_paths, InsertMode::Append));
                }
            });
        }
        self.selected
            .retain(|index| *index < self.state.playlist.len());
        if self
            .selection_anchor
            .is_some_and(|index| index >= self.state.playlist.len())
        {
            self.selection_anchor = None;
        }
        ctx.request_repaint_after(Duration::from_millis(100));

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open files…").clicked() {
                        self.add_dialog(InsertMode::Replace, AddKind::Files);
                        ui.close_menu();
                    }
                    if ui.button("Add files to end…").clicked() {
                        self.add_dialog(InsertMode::Append, AddKind::Files);
                        ui.close_menu();
                    }
                    if ui.button("Add files to beginning…").clicked() {
                        self.add_dialog(InsertMode::Prepend, AddKind::Files);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Open playlist…").clicked() {
                        self.add_dialog(InsertMode::Replace, AddKind::Playlist);
                        ui.close_menu();
                    }
                    if ui.button("Add playlist to end…").clicked() {
                        self.add_dialog(InsertMode::Append, AddKind::Playlist);
                        ui.close_menu();
                    }
                    if ui.button("Add playlist to beginning…").clicked() {
                        self.add_dialog(InsertMode::Prepend, AddKind::Playlist);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Open folder…").clicked() {
                        self.add_dialog(InsertMode::Replace, AddKind::Folder);
                        ui.close_menu();
                    }
                    if ui.button("Add folder to end…").clicked() {
                        self.add_dialog(InsertMode::Append, AddKind::Folder);
                        ui.close_menu();
                    }
                    if ui.button("Add folder to beginning…").clicked() {
                        self.add_dialog(InsertMode::Prepend, AddKind::Folder);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Clear playlist").clicked() {
                        self.send(Command::Clear);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        self.send(Command::Quit);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Playlist", |ui| {
                    if ui.button("Save playlist as M3U…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("M3U playlist", &["m3u"])
                            .set_file_name("playlist.m3u")
                            .save_file()
                        {
                            if let Err(error) = save_playlist_m3u(&self.state.playlist, &path) {
                                self.state.error = Some(error.to_string());
                            }
                        }
                        ui.close_menu();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.about_open = true;
                        ui.close_menu();
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("TeMS-Player");
            ui.horizontal(|ui| {
                ui.label("Search");
                ui.text_edit_singleline(&mut self.search);
                let mut track_number = self.selected.iter().next().map_or(1, |index| index + 1);
                if ui
                    .add(
                        egui::DragValue::new(&mut track_number)
                            .range(1..=self.state.playlist.len().max(1)),
                    )
                    .changed()
                {
                    let index = track_number.saturating_sub(1);
                    self.selected.clear();
                    self.selected.insert(index);
                    self.selection_anchor = Some(index);
                }
                if ui.button("Go to track").clicked() && !self.state.playlist.is_empty() {
                    self.send(Command::GoTo(
                        track_number
                            .saturating_sub(1)
                            .min(self.state.playlist.len() - 1),
                    ));
                }
            });
            ui.horizontal(|ui| {
                ui.label(
                    self.state
                        .current
                        .and_then(|index| self.state.playlist.get(index))
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Nothing playing".to_string()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⏹ Stop").clicked() {
                        self.send(Command::Stop);
                    }
                    if ui
                        .button(if self.state.paused {
                            "▶ Resume"
                        } else {
                            "⏸ Pause"
                        })
                        .clicked()
                    {
                        self.send(Command::TogglePause);
                    }
                    if ui.button("⏭ Next").clicked() {
                        self.send(Command::Next);
                    }
                    if ui.button("⏮ Previous").clicked() {
                        self.send(Command::Previous);
                    }
                });
            });
            let duration = self.state.duration.max(0.0);
            let mut position = self.state.position.clamp(0.0, duration);
            let mut seek_target = None;
            ui.horizontal(|ui| {
                ui.label(format_time(position));
                let slider = ui.add_sized(
                    [ui.available_width().max(0.0), 18.0],
                    egui::Slider::new(&mut position, 0.0..=duration.max(1.0)).show_value(false),
                );
                ui.label(format_time(duration));
                if slider.dragged() || slider.drag_stopped() {
                    seek_target = Some(position);
                }
                if ui.small_button("−5s").clicked() {
                    seek_target = Some((self.state.position - 5.0).max(0.0));
                }
                if ui.small_button("+5s").clicked() {
                    seek_target = Some((self.state.position + 5.0).min(duration));
                }
            });
            if let Some(seconds) = seek_target {
                self.send(Command::Seek(seconds));
            }
            ui.horizontal(|ui| {
                ui.label("Volume");
                let mut volume = self.state.volume;
                if ui
                    .add(egui::Slider::new(&mut volume, 0.0..=2.0).show_value(true))
                    .changed()
                {
                    self.send(Command::Volume(volume));
                }
                ui.label("Crossfade");
                let mut crossfade_seconds = self.state.crossfade_seconds;
                if ui
                    .add(egui::Slider::new(&mut crossfade_seconds, 0.0..=30.0).suffix(" s"))
                    .changed()
                {
                    self.send(Command::Crossfade(crossfade_seconds));
                }
                if ui.selectable_label(self.state.shuffle, "Shuffle").clicked() {
                    self.send(Command::Shuffle);
                }
                if ui
                    .button(format!(
                        "Repeat: {}",
                        match self.state.repeat {
                            RepeatMode::Off => "off",
                            RepeatMode::All => "all",
                            RepeatMode::One => "one",
                        }
                    ))
                    .clicked()
                {
                    self.send(Command::Repeat);
                }
                ui.label(format!("{} tracks", self.state.playlist.len()));
            });
            ui.separator();
            if let Some(metadata) = &self.state.metadata {
                ui.label(metadata);
            }
            if !self.selected.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(format!("{} selected", self.selected.len()));
                    if ui.button("Move after current track").clicked() {
                        let mut indices: Vec<_> = self.selected.iter().copied().collect();
                        indices.sort_unstable();
                        self.send(Command::MoveAfterCurrentMany(indices));
                    }
                    if ui.button("Remove selected").clicked() {
                        let mut indices: Vec<_> = self.selected.iter().copied().collect();
                        indices.sort_unstable();
                        self.send(Command::RemoveMany(indices));
                        self.selected.clear();
                        self.selection_anchor = None;
                    }
                });
            }
            let search = self.search.to_lowercase();
            let visible_tracks: Vec<_> = self
                .state
                .playlist
                .iter()
                .enumerate()
                .filter(|(index, path)| {
                    search.is_empty()
                        || path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&search)
                        || self.state.current == Some(*index)
                })
                .map(|(index, path)| (index, path.clone()))
                .collect();
            egui::ScrollArea::vertical().show_rows(ui, 22.0, visible_tracks.len(), |ui, rows| {
                for row in rows {
                    let (index, path) = &visible_tracks[row];
                    let index = *index;
                    let path = path.clone();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    let label = format!("{:03}  {}", index + 1, name);
                    let response = ui.selectable_label(
                        self.state.current == Some(index) || self.selected.contains(&index),
                        label,
                    );
                    if response.clicked() {
                        let (ctrl, shift) = ui.input(|input| {
                            (
                                input.modifiers.ctrl || input.modifiers.command,
                                input.modifiers.shift,
                            )
                        });
                        if shift {
                            let anchor = self.selection_anchor.unwrap_or(index);
                            self.selected.clear();
                            for selected in anchor.min(index)..=anchor.max(index) {
                                self.selected.insert(selected);
                            }
                        } else if ctrl {
                            if !self.selected.insert(index) {
                                self.selected.remove(&index);
                            }
                            self.selection_anchor = Some(index);
                        } else {
                            self.selected.clear();
                            self.selected.insert(index);
                            self.selection_anchor = Some(index);
                        }
                    }
                    if response.double_clicked() {
                        self.send(Command::Play(index));
                    }
                    response.dnd_set_drag_payload(index);
                    if let Some(dragged_index) = response.dnd_hover_payload::<usize>() {
                        if *dragged_index != index {
                            ui.painter().rect_stroke(
                                response.rect,
                                0.0,
                                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                            );
                        }
                    }
                    if let Some(dragged_index) = response.dnd_release_payload::<usize>() {
                        if *dragged_index != index {
                            self.send(Command::Move(*dragged_index, index));
                        }
                    }
                    response.context_menu(|ui| {
                        let mut indices: Vec<_> = if self.selected.contains(&index) {
                            self.selected.iter().copied().collect()
                        } else {
                            vec![index]
                        };
                        indices.sort_unstable();
                        if ui.button("Play").clicked() {
                            self.send(Command::Play(index));
                            ui.close_menu();
                        }
                        if ui.button("Move after current track").clicked() {
                            if indices.len() == 1 {
                                self.send(Command::MoveAfterCurrent(index));
                            } else {
                                self.send(Command::MoveAfterCurrentMany(indices.clone()));
                            }
                            ui.close_menu();
                        }
                        if ui.button("Remove from playlist").clicked() {
                            if indices.len() == 1 {
                                self.send(Command::Remove(index));
                            } else {
                                self.send(Command::RemoveMany(indices));
                            }
                            ui.close_menu();
                        }
                        if ui.button("Show file location").clicked() {
                            let _ = open_location(&path);
                            ui.close_menu();
                        }
                    });
                }
            });
            if let Some(error) = &self.state.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });

        if self.about_open {
            egui::Window::new("About TeMS-Player")
                .open(&mut self.about_open)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.heading("TeMS-Player 2.1");
                    ui.label("Author: Philippe TEMESI");
                    ui.hyperlink_to("Website: https://www.tems.be", "https://www.tems.be");
                    ui.label("Year: 2026");
                });
        }
    }
}

fn open_location(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(path.parent().unwrap_or(path))
        .spawn()?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(path.parent().unwrap_or(path))
        .spawn()?;
    Ok(())
}

struct LoadedAudio {
    samples: Arc<Vec<f32>>,
    sample_rate: u32,
    channels: u16,
    duration: f64,
    metadata: String,
}

struct FadingPlayback {
    handle: thread::JoinHandle<Result<()>>,
    stop: Arc<AtomicBool>,
    gain: Arc<Mutex<f32>>,
    incoming_gain: Arc<Mutex<f32>>,
    started: Instant,
    duration: f32,
}

fn stop_fading_playback(fading: &mut Option<FadingPlayback>) {
    if let Some(previous) = fading.take() {
        previous.stop.store(true, Ordering::Relaxed);
        let _ = previous.handle.join();
    }
}

fn decode_track(path: &Path) -> Result<LoadedAudio> {
    let info = get_file_info(path)?;
    let (samples, sample_rate, channels) = if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("opus"))
    {
        load_opus_file(path)?
    } else {
        load_audio_file(path)?
    };
    Ok(LoadedAudio {
        samples: Arc::new(samples),
        sample_rate,
        channels,
        duration: info.duration_secs,
        metadata: format!(
            "{} · {} Hz · {} channels · {} · {} kbps",
            info.codec,
            info.sample_rate,
            info.channels,
            format_time(info.duration_secs),
            info.bitrate.unwrap_or(0)
        ),
    })
}

fn save_playlist_m3u(playlist: &[PathBuf], path: &Path) -> Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "#EXTM3U")?;
    for track in playlist {
        writeln!(writer, "{}", track.display())?;
    }
    writer.flush()?;
    Ok(())
}

fn playback_worker(
    mut playlist: Vec<PathBuf>,
    rx: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Snapshot>,
) {
    let volume = Arc::new(Mutex::new(1.0));
    let mut current: Option<usize> = if playlist.is_empty() { None } else { Some(0) };
    let mut shuffle = false;
    let mut repeat = RepeatMode::Off;
    let mut paused = Arc::new(AtomicBool::new(false));
    let mut stop = Arc::new(AtomicBool::new(false));
    let mut pos = Arc::new(AtomicUsize::new(0));
    let mut length = 0usize;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut duration = 0.0;
    let mut codec_metadata = None;
    let mut crossfade_seconds = 0.0f32;
    let mut upcoming: Option<usize> = None;
    let mut force_transition = false;
    let mut active_audio: Option<Arc<LoadedAudio>> = None;
    let mut play_handle: Option<thread::JoinHandle<Result<()>>> = None;
    let mut playback_gain = Arc::new(Mutex::new(1.0));
    let mut fading_playback: Option<FadingPlayback> = None;
    let mut stop_fade_started: Option<(Instant, f32)> = None;
    let mut needs_load = current.is_some();
    let mut running = true;
    let (decode_tx, decode_rx) = mpsc::channel::<PathBuf>();
    let (decoded_tx, decoded_rx) = mpsc::channel::<(PathBuf, Result<Arc<LoadedAudio>, String>)>();
    thread::spawn(move || {
        while let Ok(path) = decode_rx.recv() {
            let decoded = decode_track(&path)
                .map(Arc::new)
                .map_err(|error| error.to_string());
            if decoded_tx.send((path, decoded)).is_err() {
                break;
            }
        }
    });
    let mut decoded_cache = HashMap::<PathBuf, Result<Arc<LoadedAudio>, String>>::new();
    let mut decode_requested = HashSet::<PathBuf>::new();

    while running {
        if let Some(fading) = fading_playback.as_ref() {
            let progress =
                (fading.started.elapsed().as_secs_f32() / fading.duration).clamp(0.0, 1.0);
            *fading.gain.lock().unwrap() = (progress * std::f32::consts::FRAC_PI_2).cos();
            *fading.incoming_gain.lock().unwrap() = (progress * std::f32::consts::FRAC_PI_2).sin();
            if progress >= 1.0 {
                stop_fading_playback(&mut fading_playback);
            }
        }
        while let Ok((path, decoded)) = decoded_rx.try_recv() {
            decode_requested.remove(&path);
            if current.is_some() {
                decoded_cache.insert(path, decoded);
            }
        }
        if upcoming.is_none() && !needs_load && active_audio.is_some() {
            upcoming = current.and_then(|index| {
                if repeat == RepeatMode::One {
                    Some(index)
                } else {
                    next_index(Some(index), playlist.len(), shuffle, repeat)
                }
            });
        }
        if let Some(index) = current {
            let mut keep = [index.checked_sub(1), Some(index), index.checked_add(1)]
                .into_iter()
                .flatten()
                .filter_map(|index| playlist.get(index).cloned())
                .collect::<HashSet<_>>();
            if let Some(next) = upcoming {
                if let Some(path) = playlist.get(next) {
                    keep.insert(path.clone());
                }
            }
            decoded_cache.retain(|path, _| keep.contains(path));
        } else {
            decoded_cache.clear();
        }
        if let Some(index) = current.filter(|&index| index < playlist.len()) {
            let path = playlist[index].clone();
            if !decoded_cache.contains_key(&path) && decode_requested.insert(path.clone()) {
                let _ = decode_tx.send(path.clone());
            }
            if needs_load {
                if let Some(loaded) = decoded_cache.get(&path).cloned() {
                    needs_load = false;
                    match loaded {
                        Ok(loaded) => {
                            codec_metadata = Some(loaded.metadata.clone());
                            let playback_samples = loaded.samples.clone();
                            let playback_rate = loaded.sample_rate;
                            let playback_channels = loaded.channels;
                            let is_crossfade = crossfade_seconds > 0.0
                                && upcoming.is_some()
                                && active_audio.is_some()
                                && play_handle.is_some();
                            if is_crossfade {
                                stop_fading_playback(&mut fading_playback);
                                let remaining = length.saturating_sub(pos.load(Ordering::Relaxed))
                                    as f64
                                    / (sample_rate.max(1) as f64 * channels.max(1) as f64);
                                let fade_duration =
                                    crossfade_seconds.min(remaining as f32).max(0.01);
                                let outgoing_gain = playback_gain.clone();
                                let outgoing_stop = stop.clone();
                                let incoming_gain = Arc::new(Mutex::new(0.0));
                                fading_playback = play_handle.take().map(|handle| FadingPlayback {
                                    handle,
                                    stop: outgoing_stop,
                                    gain: outgoing_gain,
                                    incoming_gain: incoming_gain.clone(),
                                    started: Instant::now(),
                                    duration: fade_duration,
                                });
                                playback_gain = incoming_gain;
                            } else {
                                stop_fading_playback(&mut fading_playback);
                                if let Some(previous_handle) = play_handle.take() {
                                    stop.store(true, Ordering::Relaxed);
                                    let _ = previous_handle.join();
                                }
                                playback_gain = Arc::new(Mutex::new(1.0));
                            }
                            active_audio = Some(loaded.clone());
                            upcoming = None;
                            force_transition = false;
                            duration = loaded.duration;
                            sample_rate = playback_rate;
                            channels = playback_channels;
                            length = playback_samples.len();
                            pos = Arc::new(AtomicUsize::new(0));
                            stop = Arc::new(AtomicBool::new(false));
                            if !is_crossfade {
                                paused = Arc::new(AtomicBool::new(false));
                            }
                            let audio_samples = playback_samples;
                            let audio_stop = stop.clone();
                            let audio_pos = pos.clone();
                            let audio_pause = paused.clone();
                            let audio_volume = volume.clone();
                            let audio_gain = playback_gain.clone();
                            play_handle = Some(thread::spawn(move || {
                                play_audio(
                                    audio_samples,
                                    playback_rate,
                                    playback_channels,
                                    super::PlaybackVolume {
                                        global: audio_volume,
                                        gain: audio_gain,
                                    },
                                    audio_stop,
                                    audio_pos,
                                    audio_pause,
                                )
                            }));
                        }
                        Err(error) => {
                            duration = 0.0;
                            length = 0;
                            let _ = updates.send(Snapshot {
                                playlist: playlist.clone(),
                                current,
                                position: 0.0,
                                duration,
                                volume: *volume.lock().unwrap(),
                                crossfade_seconds,
                                paused: false,
                                shuffle,
                                repeat,
                                error: Some(error),
                                metadata: codec_metadata.clone(),
                            });
                            current = next_index(current, playlist.len(), shuffle, repeat);
                            needs_load = current.is_some();
                        }
                    }
                }
            }
        } else if current.is_none() {
            needs_load = false;
        }
        if let Some(next) = upcoming {
            if let Some(path) = playlist.get(next) {
                if !decoded_cache.contains_key(path) && decode_requested.insert(path.clone()) {
                    let _ = decode_tx.send(path.clone());
                }
            }
        }
        if let Some((fade_started, initial_volume)) = stop_fade_started {
            let elapsed = fade_started.elapsed().as_secs_f32();
            let fade_duration = crossfade_seconds.max(0.01);
            *volume.lock().unwrap() =
                initial_volume * (1.0 - elapsed / fade_duration).clamp(0.0, 1.0);
            if elapsed >= fade_duration {
                stop_fading_playback(&mut fading_playback);
                stop.store(true, Ordering::Relaxed);
                if let Some(handle) = play_handle.take() {
                    let _ = handle.join();
                }
                stop_fade_started = None;
                active_audio = None;
                current = None;
                length = 0;
                duration = 0.0;
                codec_metadata = None;
                *volume.lock().unwrap() = initial_volume;
            }
        }
        if crossfade_seconds > 0.0
            && stop_fade_started.is_none()
            && !needs_load
            && play_handle.is_some()
        {
            if let Some(_active) = active_audio.as_ref() {
                let target = if repeat == RepeatMode::One {
                    current
                } else {
                    upcoming
                };
                let playback_position = pos.load(Ordering::Relaxed);
                let frames_left =
                    length.saturating_sub(playback_position) / channels.max(1) as usize;
                let fade_frames = (crossfade_seconds * sample_rate as f32) as usize;
                let required_remaining = if force_transition { 0 } else { fade_frames };
                if force_transition || frames_left <= required_remaining {
                    if let Some(target_index) = target {
                        if let Some(path) = playlist.get(target_index) {
                            upcoming = Some(target_index);
                            match decoded_cache.get(path) {
                                Some(Ok(_)) => {
                                    force_transition = false;
                                    current = Some(target_index);
                                    needs_load = true;
                                }
                                Some(Err(_)) => {
                                    upcoming = None;
                                    force_transition = false;
                                    current = next_index(current, playlist.len(), shuffle, repeat);
                                    needs_load = current.is_some();
                                }
                                None => {}
                            }
                        }
                    }
                }
            }
        }

        let current_position = if sample_rate > 0 && channels > 0 {
            pos.load(Ordering::Relaxed) as f64 / (sample_rate as f64 * channels as f64)
        } else {
            0.0
        };
        let _ = updates.send(Snapshot {
            playlist: playlist.clone(),
            current,
            position: current_position,
            duration,
            volume: *volume.lock().unwrap(),
            crossfade_seconds,
            paused: paused.load(Ordering::Relaxed),
            shuffle,
            repeat,
            error: None,
            metadata: codec_metadata.clone(),
        });

        let command_timeout = if fading_playback.is_some() {
            Duration::from_millis(5)
        } else {
            Duration::from_millis(100)
        };
        match rx.recv_timeout(command_timeout) {
            Ok(Command::GoTo(index)) if index < playlist.len() => {
                stop_fading_playback(&mut fading_playback);
                upcoming = None;
                active_audio = None;
                stop.store(true, Ordering::Relaxed);
                current = Some(index);
                needs_load = true;
            }
            Ok(Command::Quit) => {
                stop_fading_playback(&mut fading_playback);
                active_audio = None;
                stop.store(true, Ordering::Relaxed);
                running = false;
            }
            Ok(Command::Add(paths, mode)) => {
                if mode == InsertMode::Replace {
                    stop_fading_playback(&mut fading_playback);
                    upcoming = None;
                    active_audio = None;
                    stop.store(true, Ordering::Relaxed);
                    playlist = paths;
                    current = if playlist.is_empty() { None } else { Some(0) };
                    length = 0;
                    duration = 0.0;
                    codec_metadata = None;
                    decoded_cache.clear();
                    decode_requested.clear();
                    needs_load = current.is_some();
                } else if mode == InsertMode::Prepend {
                    let count = paths.len();
                    playlist.splice(0..0, paths);
                    if let Some(index) = current {
                        current = Some(index + count);
                    }
                    decoded_cache.clear();
                    decode_requested.clear();
                } else {
                    playlist.extend(paths);
                }
                if current.is_none() && !playlist.is_empty() {
                    current = Some(0);
                    needs_load = true;
                }
            }
            Ok(Command::Remove(index)) if index < playlist.len() => {
                remove_playlist_indices(
                    &mut playlist,
                    &[index],
                    &mut current,
                    &mut upcoming,
                    &mut active_audio,
                    &mut stop,
                    &mut needs_load,
                );
                force_transition = false;
                decoded_cache.clear();
                decode_requested.clear();
            }
            Ok(Command::RemoveMany(indices)) => {
                remove_playlist_indices(
                    &mut playlist,
                    &indices,
                    &mut current,
                    &mut upcoming,
                    &mut active_audio,
                    &mut stop,
                    &mut needs_load,
                );
                force_transition = false;
                decoded_cache.clear();
                decode_requested.clear();
            }
            Ok(Command::Clear) => {
                stop_fading_playback(&mut fading_playback);
                upcoming = None;
                active_audio = None;
                stop.store(true, Ordering::Relaxed);
                playlist.clear();
                decoded_cache.clear();
                decode_requested.clear();
                current = None;
                needs_load = false;
                length = 0;
                duration = 0.0;
                codec_metadata = None;
            }
            Ok(Command::Move(from, to)) if from < playlist.len() && to < playlist.len() => {
                if from != to {
                    let path = playlist.remove(from);
                    playlist.insert(to, path);
                    current = current.map(|index| {
                        if index == from {
                            to
                        } else if from < index && index <= to {
                            index - 1
                        } else if to <= index && index < from {
                            index + 1
                        } else {
                            index
                        }
                    });
                    upcoming = upcoming.map(|index| {
                        if index == from {
                            to
                        } else if from < index && index <= to {
                            index - 1
                        } else if to <= index && index < from {
                            index + 1
                        } else {
                            index
                        }
                    });
                    decoded_cache.clear();
                    decode_requested.clear();
                    needs_load = play_handle.is_none() && current.is_some();
                }
            }
            Ok(Command::MoveAfterCurrent(index)) if index < playlist.len() => {
                reorder_after_current(&mut playlist, &mut current, &mut upcoming, &[index]);
                decoded_cache.clear();
                decode_requested.clear();
            }
            Ok(Command::MoveAfterCurrentMany(indices)) => {
                reorder_after_current(&mut playlist, &mut current, &mut upcoming, &indices);
                decoded_cache.clear();
                decode_requested.clear();
            }
            Ok(Command::Play(index)) if index < playlist.len() => {
                stop_fading_playback(&mut fading_playback);
                upcoming = None;
                active_audio = None;
                stop.store(true, Ordering::Relaxed);
                current = Some(index);
                needs_load = true;
            }
            Ok(Command::TogglePause) if current.is_some() => {
                paused.store(!paused.load(Ordering::Relaxed), Ordering::Relaxed);
            }
            Ok(Command::Stop) => {
                upcoming = None;
                force_transition = false;
                stop_fade_started = None;
                needs_load = false;
                if crossfade_seconds > 0.0 && play_handle.is_some() {
                    *playback_gain.lock().unwrap() = 1.0;
                    stop_fade_started = Some((Instant::now(), *volume.lock().unwrap()));
                } else {
                    stop_fading_playback(&mut fading_playback);
                    active_audio = None;
                    stop.store(true, Ordering::Relaxed);
                    current = None;
                    length = 0;
                    duration = 0.0;
                    codec_metadata = None;
                }
            }
            Ok(Command::Next) => {
                if crossfade_seconds > 0.0 && play_handle.is_some() {
                    upcoming = next_index(current, playlist.len(), shuffle, repeat);
                    force_transition = true;
                } else {
                    upcoming = None;
                    force_transition = false;
                    active_audio = None;
                    stop.store(true, Ordering::Relaxed);
                }
                if crossfade_seconds == 0.0 || play_handle.is_none() {
                    current = next_index(current, playlist.len(), shuffle, repeat);
                    needs_load = current.is_some();
                } else {
                    needs_load = false;
                }
            }
            Ok(Command::Previous) => {
                if crossfade_seconds > 0.0 && play_handle.is_some() {
                    upcoming = current.map(|index| {
                        if index == 0 && repeat == RepeatMode::All {
                            playlist.len().saturating_sub(1)
                        } else {
                            index.saturating_sub(1)
                        }
                    });
                    force_transition = true;
                } else {
                    upcoming = None;
                    force_transition = false;
                    active_audio = None;
                    stop.store(true, Ordering::Relaxed);
                }
                if crossfade_seconds == 0.0 || play_handle.is_none() {
                    current = current.map(|index| {
                        if index == 0 && repeat == RepeatMode::All {
                            playlist.len().saturating_sub(1)
                        } else {
                            index.saturating_sub(1)
                        }
                    });
                    needs_load = current.is_some();
                } else {
                    needs_load = false;
                }
            }
            Ok(Command::Seek(seconds)) if sample_rate > 0 && channels > 0 => {
                let sample = (seconds.max(0.0) * sample_rate as f64 * channels as f64) as usize;
                pos.store(sample.min(length), Ordering::Relaxed);
            }
            Ok(Command::Volume(value)) => {
                *volume.lock().unwrap() = value.clamp(0.0, 2.0);
            }
            Ok(Command::Crossfade(value)) => {
                crossfade_seconds = value.clamp(0.0, 30.0);
                if crossfade_seconds == 0.0 {
                    if let Some(index) = upcoming.take() {
                        current = Some(index);
                        active_audio = None;
                        force_transition = false;
                        stop.store(true, Ordering::Relaxed);
                        needs_load = true;
                    }
                }
            }
            Ok(Command::Shuffle) => {
                shuffle = !shuffle;
                if !force_transition {
                    upcoming = None;
                }
            }
            Ok(Command::Repeat) => {
                repeat = match repeat {
                    RepeatMode::Off => RepeatMode::All,
                    RepeatMode::All => RepeatMode::One,
                    RepeatMode::One => RepeatMode::Off,
                };
                if !force_transition {
                    upcoming = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => running = false,
            _ => {}
        }

        if stop_fade_started.is_none()
            && (stop.load(Ordering::Relaxed)
                || (length > 0 && pos.load(Ordering::Relaxed) >= length))
            && play_handle.is_some()
        {
            if let Some(handle) = play_handle.take() {
                let _ = handle.join();
            }
            if !stop.load(Ordering::Relaxed) && length > 0 && !needs_load {
                current = if repeat == RepeatMode::One {
                    current
                } else {
                    upcoming.or_else(|| next_index(current, playlist.len(), shuffle, repeat))
                };
                needs_load = current.is_some();
            }
            if stop.load(Ordering::Relaxed) {
                stop.store(false, Ordering::Relaxed);
            }
            if current.is_none() {
                length = 0;
                duration = 0.0;
            }
        }
    }
    stop_fading_playback(&mut fading_playback);
    stop.store(true, Ordering::Relaxed);
    if let Some(handle) = play_handle {
        let _ = handle.join();
    }
}

fn reorder_after_current(
    playlist: &mut Vec<PathBuf>,
    current: &mut Option<usize>,
    upcoming: &mut Option<usize>,
    indices: &[usize],
) {
    let Some(current_index) = current.filter(|index| *index < playlist.len()) else {
        return;
    };
    let selected: HashSet<_> = indices
        .iter()
        .copied()
        .filter(|index| *index < playlist.len() && *index != current_index)
        .collect();
    if selected.is_empty() {
        return;
    }
    let upcoming_old = *upcoming;
    let mut entries: Vec<_> = playlist.drain(..).enumerate().collect();
    let moving: Vec<_> = entries
        .iter()
        .filter(|(index, _)| selected.contains(index))
        .cloned()
        .collect();
    entries.retain(|(index, _)| !selected.contains(index));
    let current_position = entries
        .iter()
        .position(|(old_index, _)| *old_index == current_index)
        .unwrap_or(0);
    let insertion = current_position + 1;
    entries.splice(insertion..insertion, moving);
    *current = entries
        .iter()
        .position(|(old_index, _)| *old_index == current_index);
    *upcoming = upcoming_old.and_then(|index| {
        entries
            .iter()
            .position(|(old_index, _)| *old_index == index)
    });
    *playlist = entries.into_iter().map(|(_, path)| path).collect();
}

fn remove_playlist_indices(
    playlist: &mut Vec<PathBuf>,
    indices: &[usize],
    current: &mut Option<usize>,
    upcoming: &mut Option<usize>,
    active_audio: &mut Option<Arc<LoadedAudio>>,
    stop: &mut Arc<AtomicBool>,
    needs_load: &mut bool,
) {
    let removed: HashSet<_> = indices
        .iter()
        .copied()
        .filter(|index| *index < playlist.len())
        .collect();
    if removed.is_empty() {
        return;
    }
    let current_removed = current.is_some_and(|index| removed.contains(&index));
    let current_before = current.unwrap_or(0);
    let upcoming_before = *upcoming;
    let removed_before_current = removed
        .iter()
        .filter(|index| **index < current_before)
        .count();
    *playlist = playlist
        .drain(..)
        .enumerate()
        .filter_map(|(index, path)| (!removed.contains(&index)).then_some(path))
        .collect();
    *current = if current_removed {
        (!playlist.is_empty()).then_some(current_before.min(playlist.len() - 1))
    } else {
        current.map(|index| index - removed_before_current)
    };
    *upcoming = upcoming_before.and_then(|index| {
        if removed.contains(&index) {
            None
        } else {
            Some(
                index
                    - removed
                        .iter()
                        .filter(|removed_index| **removed_index < index)
                        .count(),
            )
        }
    });
    if current_removed {
        *active_audio = None;
        stop.store(true, Ordering::Relaxed);
        *needs_load = current.is_some();
    }
}

fn next_index(
    current: Option<usize>,
    length: usize,
    shuffle: bool,
    repeat: RepeatMode,
) -> Option<usize> {
    let index = current?;
    if length == 0 {
        return None;
    }
    if shuffle {
        return Some(super::get_random_index(index, length));
    }
    if index + 1 < length {
        Some(index + 1)
    } else if repeat == RepeatMode::All {
        Some(0)
    } else {
        None
    }
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::{remove_playlist_indices, reorder_after_current};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn paths(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|index| PathBuf::from(format!("{index}.wav")))
            .collect()
    }

    #[test]
    fn moving_current_track_does_not_change_its_identity() {
        let mut playlist = paths(4);
        let mut current = Some(2);
        let mut upcoming = Some(3);
        reorder_after_current(&mut playlist, &mut current, &mut upcoming, &[2]);
        assert_eq!(playlist, paths(4));
        assert_eq!(current, Some(2));
        assert_eq!(upcoming, Some(3));
    }

    #[test]
    fn moving_multiple_tracks_preserves_order_and_current_identity() {
        let mut playlist = paths(5);
        let mut current = Some(2);
        let mut upcoming = Some(4);
        reorder_after_current(&mut playlist, &mut current, &mut upcoming, &[0, 4]);
        assert_eq!(
            playlist,
            vec![
                paths(5)[1].clone(),
                paths(5)[2].clone(),
                paths(5)[0].clone(),
                paths(5)[4].clone(),
                paths(5)[3].clone()
            ]
        );
        assert_eq!(current, Some(1));
        assert_eq!(upcoming, Some(3));
    }

    #[test]
    fn removing_multiple_tracks_keeps_current_track_without_restarting_it() {
        let mut playlist = paths(5);
        let mut current = Some(3);
        let mut upcoming = Some(4);
        let mut active_audio = None;
        let mut stop = Arc::new(AtomicBool::new(false));
        let mut needs_load = false;
        remove_playlist_indices(
            &mut playlist,
            &[0, 2],
            &mut current,
            &mut upcoming,
            &mut active_audio,
            &mut stop,
            &mut needs_load,
        );
        assert_eq!(
            playlist,
            vec![
                paths(5)[1].clone(),
                paths(5)[3].clone(),
                paths(5)[4].clone()
            ]
        );
        assert_eq!(current, Some(1));
        assert_eq!(upcoming, Some(2));
        assert!(!stop.load(Ordering::Relaxed));
        assert!(!needs_load);
    }
}
