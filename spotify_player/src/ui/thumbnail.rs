//! Cover thumbnails rendered next to list rows.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use image::DynamicImage;
use parking_lot::Mutex;
use ratatui::{layout::Rect, Frame};
use ratatui_image::picker::{Picker, ProtocolType};

use super::cover_image::CoverImage;
use crate::config;

/// Longest side, in pixels, that downloaded thumbnails are downscaled to.
const THUMBNAIL_PIXELS: u32 = 96;
const NUM_WORKERS: usize = 4;
/// Number of prepared thumbnails above which off-screen ones are evicted.
const MAX_PREPARED: usize = 256;

enum LoadState {
    Loading,
    Ready(DynamicImage),
    Failed,
}

/// Loads, caches and renders list thumbnails.
///
/// Images are fetched by background worker threads (spawned on first use) from Spotify's image
/// CDN, which is not subject to the Web API's rate limits.
#[derive(Default)]
pub struct Thumbnails {
    loaded: Arc<Mutex<HashMap<String, LoadState>>>,
    requests: Option<flume::Sender<String>>,
    prepared: HashMap<String, CoverImage>,
    prepared_size: (u16, u16),
    /// Thumbnail URL rendered at each area in the previous and the current frame.
    /// Used to redraw cursor-anchored (iTerm2) images only when a row's content changes.
    prev_placements: HashMap<Rect, String>,
    placements: HashMap<Rect, String>,
}

impl std::fmt::Debug for Thumbnails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumbnails")
            .field("prepared", &self.prepared.len())
            .finish_non_exhaustive()
    }
}

impl Thumbnails {
    /// Must be called once at the start of every frame.
    pub fn begin_frame(&mut self) {
        self.prev_placements = std::mem::take(&mut self.placements);
        if self.prepared.len() > MAX_PREPARED {
            let visible = &self.prev_placements;
            self.prepared
                .retain(|url, _| visible.values().any(|u| u == url));
        }
    }

    /// Force every thumbnail to be redrawn, e.g. after the terminal was cleared.
    pub fn invalidate(&mut self) {
        self.prev_placements.clear();
    }

    /// Render the thumbnails of a list's visible rows into the left edge of `area`.
    ///
    /// `urls[i]` is the thumbnail of the list's `i`-th item and `offset` is the index of the
    /// first visible item.
    pub fn render_list(
        &mut self,
        picker: &Picker,
        frame: &mut Frame,
        area: Rect,
        offset: usize,
        urls: &[Option<String>],
    ) {
        let Some((width, height)) = thumbnail_size(picker) else {
            return;
        };
        if self.prepared_size != (width, height) {
            self.prepared.clear();
            self.prepared_size = (width, height);
        }

        let rows = (area.height / height) as usize;
        for (row, url) in urls.iter().skip(offset).take(rows).enumerate() {
            let Some(url) = url else { continue };
            let rect = Rect {
                x: area.x,
                y: area.y + row as u16 * height,
                width: width.min(area.width),
                height,
            };
            self.render_one(picker, frame, rect, url);
        }
    }

    fn render_one(&mut self, picker: &Picker, frame: &mut Frame, rect: Rect, url: &str) {
        if !self.prepared.contains_key(url) {
            let Some(img) = self.take_loaded(url) else {
                return;
            };
            match CoverImage::new(picker, &img, rect) {
                Ok(cover) => {
                    self.prepared.insert(url.to_string(), cover);
                }
                Err(err) => {
                    tracing::error!("Failed to prepare thumbnail {url}: {err:#}");
                    self.loaded
                        .lock()
                        .insert(url.to_string(), LoadState::Failed);
                    return;
                }
            }
        }

        let cover = self.prepared.get_mut(url).expect("prepared thumbnail");
        if self.prev_placements.get(&rect).map(String::as_str) != Some(url) {
            cover.invalidate();
        }
        cover.render(frame, rect);
        self.placements.insert(rect, url.to_string());
    }

    /// Take a downloaded image out of the loader, requesting it if it hasn't been yet.
    fn take_loaded(&mut self, url: &str) -> Option<DynamicImage> {
        let mut loaded = self.loaded.lock();
        match loaded.remove(url) {
            Some(LoadState::Ready(img)) => Some(img),
            Some(state) => {
                loaded.insert(url.to_string(), state);
                None
            }
            None => {
                loaded.insert(url.to_string(), LoadState::Loading);
                drop(loaded);
                if self
                    .requests
                    .get_or_insert_with(|| spawn_workers(&self.loaded))
                    .send(url.to_string())
                    .is_err()
                {
                    tracing::error!("Thumbnail workers are not running");
                }
                None
            }
        }
    }
}

/// Size of a thumbnail in terminal cells, or `None` if thumbnails are disabled.
pub fn thumbnail_size(picker: &Picker) -> Option<(u16, u16)> {
    let height = config::get_config().app_config.list_thumbnail_height as u16;
    // a block-character fallback is too coarse to show anything at thumbnail size
    if height == 0 || picker.protocol_type() == ProtocolType::Halfblocks {
        return None;
    }
    let font_size = picker.font_size();
    let width = (height * font_size.height).div_ceil(font_size.width.max(1));
    Some((width, height))
}

fn spawn_workers(loaded: &Arc<Mutex<HashMap<String, LoadState>>>) -> flume::Sender<String> {
    let (tx, rx) = flume::unbounded::<String>();
    let configs = config::get_config();
    let cache_dir = configs
        .app_config
        .enable_cover_image_cache
        .then(|| configs.cache_folder.join("image").join("thumbnails"));
    if let Some(dir) = &cache_dir {
        if let Err(err) = std::fs::create_dir_all(dir) {
            tracing::error!("Failed to create thumbnail cache folder: {err:#}");
        }
    }
    let http = reqwest::blocking::Client::new();

    for i in 0..NUM_WORKERS {
        let (rx, loaded, http, cache_dir) =
            (rx.clone(), loaded.clone(), http.clone(), cache_dir.clone());
        let spawned = std::thread::Builder::new()
            .name(format!("thumbnail-{i}"))
            .spawn(move || {
                while let Ok(url) = rx.recv() {
                    let state = match load(&http, &url, cache_dir.as_deref()) {
                        Ok(img) => LoadState::Ready(img),
                        Err(err) => {
                            tracing::warn!("Failed to load thumbnail {url}: {err:#}");
                            LoadState::Failed
                        }
                    };
                    loaded.lock().insert(url, state);
                    crate::ui::request_redraw();
                }
            });
        if let Err(err) = spawned {
            tracing::error!("Failed to spawn thumbnail worker: {err:#}");
        }
    }
    tx
}

fn load(
    http: &reqwest::blocking::Client,
    url: &str,
    cache_dir: Option<&Path>,
) -> Result<DynamicImage> {
    let path = cache_dir.and_then(|dir| cache_path(dir, url));
    let bytes = if let Some(path) = path.as_ref().filter(|p| p.exists()) {
        std::fs::read(path).context("read cached thumbnail")?
    } else {
        let bytes = http
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .context("download thumbnail")?
            .bytes()
            .context("read thumbnail body")?
            .to_vec();
        if let Some(path) = &path {
            if let Err(err) = std::fs::write(path, &bytes) {
                tracing::warn!("Failed to cache thumbnail at {}: {err:#}", path.display());
            }
        }
        bytes
    };
    let img = image::load_from_memory(&bytes).context("decode thumbnail")?;
    Ok(img.thumbnail(THUMBNAIL_PIXELS, THUMBNAIL_PIXELS))
}

/// Spotify image URLs end in a unique image id, which doubles as the cache file name.
fn cache_path(dir: &Path, url: &str) -> Option<PathBuf> {
    let id = url.rsplit('/').next()?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| dir.join(id))
}
