//! Bound decoded-image residency across navigation; disk artwork remains reusable.
use gpui_kit::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
pub struct ResidentImages {
    cache: Entity<RetainAllImageCache>,
    recent: HashMap<Resource, (u64, usize)>,
    clock: u64,
    bytes: usize,
    owner: WeakEntity<Self>,
    used_by_view: HashMap<EntityId, HashSet<Resource>>,
    sweep_pending: bool,
}
impl ResidentImages {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let cache = RetainAllImageCache::new(cx);
        cx.new(|cx| Self {
            cache,
            recent: HashMap::new(),
            clock: 0,
            bytes: 0,
            owner: cx.weak_entity(),
            used_by_view: HashMap::new(),
            sweep_pending: false,
        })
    }
    /// Cached views keep their atlas tiles until that view renders again.
    pub fn begin_view(&mut self, view: EntityId) {
        if let Some(used) = self.used_by_view.get_mut(&view) {
            used.clear();
        }
    }
    pub fn release_view(&mut self, view: EntityId, window: &mut Window) {
        if self.used_by_view.remove(&view).is_some() {
            self.schedule_sweep(window);
        }
    }
    fn schedule_sweep(&mut self, window: &mut Window) {
        if self.sweep_pending {
            return;
        }
        self.sweep_pending = true;
        let owner = self.owner.clone();
        window.on_next_frame(move |window, cx| {
            let _ = owner.update(cx, |cache, cx| {
                cache.sweep_pending = false;
                cache.sweep(window, cx);
            });
        });
    }
    fn sweep(&mut self, window: &mut Window, cx: &mut App) {
        // The budget covers reusable history. Never evict the visible working
        // set, including images in cached views that did not repaint this frame.
        if self.recent.len() > 192 || self.bytes > 24 * 1024 * 1024 {
            let pinned: HashSet<_> = self
                .used_by_view
                .values()
                .flat_map(|used| used.iter())
                .collect();
            let mut removable: Vec<_> = self
                .recent
                .iter()
                .filter(|(key, _)| !pinned.contains(key))
                .map(|(source, (at, _))| (source.clone(), *at))
                .collect();
            removable.sort_unstable_by_key(|(_, at)| *at);
            for (oldest, _) in removable {
                if self.recent.len() <= 192 && self.bytes <= 24 * 1024 * 1024 {
                    break;
                }
                if let Some((_, bytes)) = self.recent.remove(&oldest) {
                    self.bytes -= bytes;
                }
                self.cache
                    .update(cx, |cache, cx| cache.remove(&oldest, window, cx));
            }
        }
        #[cfg(feature = "allocation-profile")]
        {
            use std::sync::atomic::Ordering::Relaxed;
            raag::diagnostics::IMAGE_BYTES.store(self.bytes, Relaxed);
            raag::diagnostics::IMAGE_COUNT.store(self.recent.len(), Relaxed);
        }
    }
}
impl ImageCache for ResidentImages {
    fn load(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.clock += 1;
        self.used_by_view
            .entry(window.current_view())
            .or_default()
            .insert(source.clone());
        let result = self
            .cache
            .update(cx, |cache, cx| cache.load(source, window, cx));
        let new_source = !self.recent.contains_key(source);
        let previous = self.recent.get(source).map_or(0, |(_, bytes)| *bytes);
        let bytes = match &result {
            Some(Ok(_)) if previous > 0 => previous,
            Some(Ok(image)) => (0..image.frame_count())
                .filter_map(|frame| image.as_bytes(frame))
                .map(<[u8]>::len)
                .sum(),
            _ => 0,
        };
        if let Some(entry) = self.recent.get_mut(source) {
            *entry = (self.clock, bytes);
        } else {
            self.recent.insert(source.clone(), (self.clock, bytes));
        }
        self.bytes = self.bytes - previous + bytes;
        if new_source || previous != bytes {
            // Wait until all visible images have registered before trimming.
            // Cache hits must not schedule frames and create an idle redraw loop.
            self.schedule_sweep(window);
        }
        result
    }
}

/// Definite artwork geometry does not need an image decode during layout.
/// Start the load in prepaint only when it intersects the current clip mask.
/// Keep a decoded cover visible while a replacement loads. A weak reference
/// preserves the shared cache's memory budget and lives only with this UI slot.
pub fn picture_keyed(
    source: impl Into<ImageSource>,
    cache: Entity<ResidentImages>,
    key: impl Into<ElementId>,
    radius: Pixels,
) -> AnyElement {
    picture_aligned(source.into(), cache, key.into(), false, radius)
}
pub fn picture_top_keyed(
    source: impl Into<ImageSource>,
    cache: Entity<ResidentImages>,
    key: impl Into<ElementId>,
) -> AnyElement {
    picture_aligned(source.into(), cache, key.into(), true, px(0.))
}
fn picture_aligned(
    source: ImageSource,
    cache: Entity<ResidentImages>,
    key: ElementId,
    align_top: bool,
    radius: Pixels,
) -> AnyElement {
    if matches!(source, ImageSource::Image(_)) {
        return img(source)
            .size_full()
            .rounded(radius)
            .object_fit(ObjectFit::Cover)
            .into_any_element();
    }
    canvas(
        move |bounds, window, cx| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return None;
            }
            let loaded = match &source {
                ImageSource::Resource(resource) => cache
                    .update(cx, |cache, cx| cache.load(resource, window, cx))
                    .and_then(Result::ok),
                ImageSource::Render(image) => Some(image.clone()),
                ImageSource::Custom(load) => load(window, cx).and_then(Result::ok),
                ImageSource::Image(_) => unreachable!(),
            };
            {
                let retained = window
                    .use_keyed_state(key.clone(), cx, |_, _| None::<std::sync::Weak<RenderImage>>);
                if let Some(image) = &loaded {
                    retained.update(cx, |previous, _| *previous = Some(Arc::downgrade(image)));
                }
                loaded.or_else(|| {
                    retained
                        .read(cx)
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                })
            }
        },
        move |bounds, image, window, _| {
            if let Some(image) = image
                && image.frame_count() > 0
            {
                let mut image_bounds = ObjectFit::Cover.get_bounds(bounds, image.size(0));
                if align_top {
                    image_bounds.origin.y = bounds.origin.y;
                }
                // GPUI's rectangular content mask does not clip rounded parents.
                let _ =
                    window.paint_image(bounds, image_bounds, Corners::all(radius), image, 0, false);
            }
        },
    )
    .size_full()
    .into_any_element()
}
