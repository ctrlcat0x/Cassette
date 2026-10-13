use super::*;

#[derive(Clone)]
pub(super) struct TrackDrag {
    pub track: Track,
    pub screen: Screen,
    pub index: usize,
}
#[derive(Clone)]
pub(super) struct PlaylistDrag(pub String);
#[derive(Clone)]
pub(super) struct FolderDrag(pub String);
pub(super) struct DragPreview(pub String);
impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_4()
            .py_2()
            .rounded_lg()
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().primary)
            .shadow_md()
            .child(self.0.clone())
    }
}

impl Raag {
    pub(super) fn set_ui_scale(&mut self, scale: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.library.ui_scale = (scale * 100.).round().clamp(75., 175.) / 100.;
        raag::i18n::set_scale(self.library.ui_scale);
        window.set_rem_size(px(16. * self.library.ui_scale));
        self.lyrics_geometry = None;
        self.apply_appearance(cx);
        self.save();
    }
    pub(super) fn localization_settings(&self, cx: &mut Context<Self>) -> Div {
        self.settings_section(
            t("Language and size"),
            t("Interface language and display scale"),
            cx,
        )
        .child(
            div().flex().flex_wrap().gap_2().children(
                [("en", "English"), ("hi", "हिन्दी"), ("es", "Español")]
                    .into_iter()
                    .map(|(code, label)| {
                        Button::new(code)
                            .label(label)
                            .selected(self.library.language == code)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.library.language = code.into();
                                raag::i18n::set_language(code);
                                gpui_kit::component::set_locale(code);
                                if let Some(system) = &this.system {
                                    system.refresh_language();
                                }
                                for (input, placeholder) in [
                                    (&this.query, "Search songs, artists, albums…"),
                                    (&this.playlist_name, "Playlist name"),
                                    (&this.organization_name, "Folder name"),
                                    (&this.smart_name, "Smart playlist name"),
                                    (&this.smart_query, "Song title, artist or album"),
                                    (&this.smart_artist, "Any artist"),
                                    (&this.smart_album, "Any album"),
                                    (&this.browse_query, "Filter this collection…"),
                                    (&this.mpv_path, "Automatic (mpv on PATH or bundled)"),
                                    (&this.ytdlp_path, "Automatic (yt-dlp on PATH or bundled)"),
                                ] {
                                    input.update(cx, |input, cx| {
                                        input.set_placeholder(t(placeholder), window, cx)
                                    });
                                }
                                this.apply_appearance(cx);
                                this.save();
                            }))
                    }),
            ),
        )
        .child(div().text_sm().child(t("Interface scale")))
        .child(
            div().flex().flex_wrap().gap_2().children(
                [0.75f32, 0.9, 1., 1.1, 1.25, 1.5, 1.75]
                    .into_iter()
                    .enumerate()
                    .map(|(i, scale)| {
                        Button::new(("interface-scale", i))
                            .label(format!("{:.0}%", scale * 100.))
                            .selected((self.library.ui_scale - scale).abs() < 0.01)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_ui_scale(scale, window, cx);
                            }))
                    }),
            ),
        )
    }

    pub(super) fn organization_controls(&self, cx: &mut Context<Self>) -> Div {
        let parent = self
            .library
            .active_folder
            .as_ref()
            .and_then(|id| self.library.folders.iter().find(|f| &f.id == id))
            .and_then(|f| f.parent.clone());
        let mut body = div().flex().flex_col().gap_3().flex_shrink_0();
        let mut folders = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                Button::new("all-folders")
                    .ghost()
                    .label(t("All playlists"))
                    .selected(self.library.active_folder.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.library.active_folder = None;
                        this.navigation_revision += 1;
                        this.save();
                        this.content_revision += 1;
                        cx.notify();
                    })),
            )
            .when(self.library.active_folder.is_some(), |row| {
                row.child(
                    Button::new("parent-folder")
                        .ghost()
                        .label(t("Parent folder"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.library.active_folder = parent.clone();
                            this.navigation_revision += 1;
                            this.save();
                            this.content_revision += 1;
                            cx.notify();
                        })),
                )
            });
        folders = folders.child(
            div()
                .id("root-folder-drop")
                .px_3()
                .py_2()
                .rounded_lg()
                .text_sm()
                .child(t("Drop here to remove from folder"))
                .drag_over::<PlaylistDrag>(|s, _, _, cx| s.bg(cx.theme().accent))
                .drag_over::<FolderDrag>(|s, _, _, cx| s.bg(cx.theme().accent))
                .on_drop(cx.listener(|this, drag: &PlaylistDrag, _, cx| {
                    this.library
                        .playlist_metadata
                        .entry(drag.0.clone())
                        .or_default()
                        .folder = None;
                    this.navigation_revision += 1;
                    this.content_revision += 1;
                    this.save();
                    cx.notify();
                }))
                .on_drop(cx.listener(|this, drag: &FolderDrag, _, cx| {
                    if let Err(error) = this.library.move_folder(&drag.0, None) {
                        this.notice = Some(error.to_string());
                    }
                    this.navigation_revision += 1;
                    this.content_revision += 1;
                    this.save();
                    cx.notify();
                })),
        );
        for folder in self
            .library
            .folders
            .iter()
            .filter(|f| f.parent == self.library.active_folder)
        {
            let selected = folder.id.clone();
            let dropped = folder.id.clone();
            let folder_target = folder.id.clone();
            let folder_drag = folder.id.clone();
            folders = folders.child(
                div()
                    .id(SharedString::from(format!("folder-drop-{}", folder.id)))
                    .on_drag(FolderDrag(folder_drag), |drag, _, _, cx| {
                        cx.new(|_| DragPreview(drag.0.clone()))
                    })
                    .drag_over::<FolderDrag>(|s, _, _, cx| {
                        s.bg(cx.theme().accent)
                            .border_1()
                            .border_color(cx.theme().primary)
                    })
                    .on_drop(cx.listener(move |this, drag: &FolderDrag, _, cx| {
                        if let Err(error) = this
                            .library
                            .move_folder(&drag.0, Some(folder_target.clone()))
                        {
                            this.notice = Some(error.to_string());
                        }
                        this.navigation_revision += 1;
                        this.content_revision += 1;
                        this.save();
                        cx.notify();
                    }))
                    .drag_over::<PlaylistDrag>(|s, _, _, cx| {
                        s.bg(cx.theme().accent)
                            .border_1()
                            .border_color(cx.theme().primary)
                    })
                    .on_drop(cx.listener(move |this, drag: &PlaylistDrag, _, cx| {
                        this.library
                            .playlist_metadata
                            .entry(drag.0.clone())
                            .or_default()
                            .folder = Some(dropped.clone());
                        this.navigation_revision += 1;
                        this.save();
                        this.content_revision += 1;
                        cx.notify();
                    }))
                    .child(
                        Button::new(SharedString::from(format!("folder-{}", folder.id)))
                            .ghost()
                            .icon(icon("folder"))
                            .label(folder.name.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.library.active_folder = Some(selected.clone());
                                if let Some(folder) =
                                    this.library.folders.iter().find(|f| f.id == selected)
                                {
                                    this.organization_name.update(cx, |input, cx| {
                                        input.set_value(folder.name.clone(), window, cx)
                                    });
                                }
                                this.navigation_revision += 1;
                                this.save();
                                this.content_revision += 1;
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some(id) = &self.library.active_folder {
            let owner = cx.weak_entity();
            let id = id.clone();
            let choices: Vec<_> = std::iter::once((None, t("Top level").to_owned()))
                .chain(
                    self.library
                        .folders
                        .iter()
                        .filter(|f| f.id != id)
                        .map(|f| (Some(f.id.clone()), f.name.clone())),
                )
                .collect();
            folders = folders.child(
                Button::new("move-folder")
                    .ghost()
                    .label(t("Move folder"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (parent, name) in &choices {
                            let owner = owner.clone();
                            let id = id.clone();
                            let parent = parent.clone();
                            menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        if let Err(error) =
                                            this.library.move_folder(&id, parent.clone())
                                        {
                                            this.notice = Some(error.to_string());
                                        }
                                        this.navigation_revision += 1;
                                        this.content_revision += 1;
                                        this.save();
                                        cx.notify();
                                    });
                                },
                            ));
                        }
                        menu
                    }),
            );
        }
        body = body.child(folders).child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .min_w(rems(12.))
                        .flex_1()
                        .child(Input::new(&self.organization_name)),
                )
                .child(
                    Button::new("create-folder")
                        .label(t("New folder"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let name = this.organization_name.read(cx).value().to_string();
                            if let Err(error) = this
                                .library
                                .create_folder(&name, this.library.active_folder.clone())
                            {
                                this.notice = Some(error.to_string());
                            }
                            this.navigation_revision += 1;
                            this.save();
                            this.content_revision += 1;
                            cx.notify();
                        })),
                )
                .when(self.library.active_folder.is_some(), |row| {
                    row.child(
                        Button::new("rename-folder")
                            .ghost()
                            .label(t("Rename folder"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let name =
                                    this.organization_name.read(cx).value().trim().to_owned();
                                if name.is_empty() || name.chars().count() > 100 {
                                    this.notice = Some(
                                        t("Enter a folder name between 1 and 100 characters.")
                                            .into(),
                                    );
                                } else if let Some(folder) =
                                    this.library.folders.iter_mut().find(|f| {
                                        Some(&f.id) == this.library.active_folder.as_ref()
                                    })
                                {
                                    folder.name = name;
                                }
                                this.navigation_revision += 1;
                                this.save();
                                this.content_revision += 1;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("remove-folder")
                            .ghost()
                            .label(t("Remove folder, keep playlists"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(id) = this.library.active_folder.clone() {
                                    this.library.remove_folder(&id);
                                }
                                this.navigation_revision += 1;
                                this.save();
                                this.content_revision += 1;
                                cx.notify();
                            })),
                    )
                })
                .child(
                    Button::new("new-smart-playlist")
                        .label(t("New smart playlist"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.editing_smart = Some(String::new());
                            this.smart_source = 0;
                            this.smart_exclude_explicit = false;
                            for input in [
                                &this.smart_name,
                                &this.smart_query,
                                &this.smart_artist,
                                &this.smart_album,
                                &this.smart_plays,
                            ] {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            this.content_revision += 1;
                            cx.notify();
                        })),
                ),
        );
        if self.editing_smart.is_some() {
            body = body.child(self.smart_editor(cx));
        }
        body
    }
    fn smart_editor(&self, cx: &mut Context<Self>) -> Div {
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg();
        for (label, input) in [
            (t("Playlist name"), &self.smart_name),
            (t("Contains text"), &self.smart_query),
            (t("Artist contains"), &self.smart_artist),
            (t("Album contains"), &self.smart_album),
            (t("Minimum plays"), &self.smart_plays),
        ] {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .child(label)
                    .child(Input::new(input)),
            );
        }
        body.child(Button::new("smart-source").label(tr!("Source: {}", [t("All library entries"), t("Liked songs"), t("Local files"), t("Recently played")][self.smart_source.min(3)])).on_click(cx.listener(|this, _, _, cx| { this.smart_source = (this.smart_source + 1) % 4; this.content_revision += 1; cx.notify(); })))
            .child(Switch::new("smart-clean").label(t("Exclude explicit songs")).checked(self.smart_exclude_explicit).on_change(cx.listener(|this, v, _, cx| { this.smart_exclude_explicit = *v; this.content_revision += 1; cx.notify(); })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t("Rules match songs in your library, synced likes and history. Membership updates automatically.")))
            .child(div().flex().gap_2()
                .child(Button::new("save-smart").primary().label(t("Save smart playlist")).on_click(cx.listener(|this, _, _, cx| {
                    let minimum = this.smart_plays.read(cx).value().trim().to_owned();
                    let minimum_plays = if minimum.is_empty() { 0 } else { match minimum.parse::<u64>() { Ok(value) => value, Err(_) => { this.notice = Some(t("Minimum plays must be a whole number.").into()); this.content_revision += 1; cx.notify(); return; } } };
                    let name = this.smart_name.read(cx).value().trim().to_owned();
                    let id = match this.editing_smart.clone().filter(|id| !id.is_empty()) { Some(id) => { if let Err(error) = this.library.rename_playlist(&id, &name) { this.notice = Some(error.to_string()); this.content_revision += 1; cx.notify(); return; } id }, None => match this.library.create_playlist(&name) { Ok(id) => id, Err(error) => { this.notice = Some(error.to_string()); this.content_revision += 1; cx.notify(); return; } } };
                    let rule = raag::organization::SmartRule { query: this.smart_query.read(cx).value().to_string(), artist: this.smart_artist.read(cx).value().to_string(), album: this.smart_album.read(cx).value().to_string(), source: this.smart_source, minimum_plays, exclude_explicit: this.smart_exclude_explicit };
                    if let Some(playlist) = this.library.playlists.iter_mut().find(|p| p.id == id) { playlist.smart = Some(rule); }
                    this.library.playlist_metadata.entry(id.clone()).or_default().folder = this.library.active_folder.clone();
                    this.editing_smart = None; this.save(); this.navigate(Screen::Collection(id, name, true), cx); this.content_revision += 1; cx.notify();
                })))
                .child(Button::new("cancel-smart").ghost().label(t("Cancel")).on_click(cx.listener(|this, _, _, cx| { this.editing_smart = None; this.content_revision += 1; cx.notify(); }))))
    }
    pub(super) fn playlist_organization_tools(&self, cx: &mut Context<Self>) -> Div {
        let Screen::Collection(id, _, local) = &self.screen else {
            return div();
        };
        let key = id.strip_prefix("VL").unwrap_or(id).to_owned();
        let current = self
            .library
            .playlist_metadata
            .get(&key)
            .and_then(|m| m.folder.clone());
        let meta_key = key.clone();
        let cover_key = key.clone();
        let remove_cover = key.clone();
        let mut row = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(
                Button::new("playlist-folder")
                    .ghost()
                    .label(tr!(
                        "Folder: {}",
                        current
                            .as_ref()
                            .and_then(|id| self.library.folders.iter().find(|f| &f.id == id))
                            .map_or(t("None"), |f| f.name.as_str())
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let folders: Vec<_> = std::iter::once(None)
                            .chain(this.library.folders.iter().map(|f| Some(f.id.clone())))
                            .collect();
                        let current = this
                            .library
                            .playlist_metadata
                            .entry(meta_key.clone())
                            .or_default();
                        let index = folders
                            .iter()
                            .position(|f| *f == current.folder)
                            .unwrap_or(0);
                        current.folder = folders[(index + 1) % folders.len()].clone();
                        this.navigation_revision += 1;
                        this.save();
                        this.content_revision += 1;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("custom-playlist-cover")
                    .ghost()
                    .label(t("Choose cover on this device…"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.choose_playlist_cover(cover_key.clone(), cx)
                    })),
            )
            .child(
                Button::new("clear-playlist-cover")
                    .ghost()
                    .label(t("Reset cover"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.library
                            .playlist_metadata
                            .entry(remove_cover.clone())
                            .or_default()
                            .cover = None;
                        this.navigation_revision += 1;
                        this.save();
                        this.content_revision += 1;
                        this.content_revision += 1;
                        cx.notify();
                    })),
            );
        if *local {
            let clean_id = id.clone();
            row = row.child(
                Button::new("clean-playlist")
                    .ghost()
                    .label(t("Remove duplicate songs"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let count = this.library.deduplicate_playlist(&clean_id);
                        this.notice = Some(tr!("Removed {count} duplicate songs"));
                        this.save();
                        this.refresh_visible();
                        this.navigation_revision += 1;
                        this.content_revision += 1;
                        cx.notify();
                    })),
            );
            if let Some(rule) = self
                .library
                .playlists
                .iter()
                .find(|p| p.id == *id)
                .and_then(|p| p.smart.clone())
            {
                let edit_id = id.clone();
                let name = self.screen.title().to_owned();
                row = row.child(
                    Button::new("edit-smart-rule")
                        .label(t("Edit smart rules"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.editing_smart = Some(edit_id.clone());
                            this.smart_source = rule.source.min(3);
                            this.smart_exclude_explicit = rule.exclude_explicit;
                            for (input, value) in [
                                (&this.smart_name, name.clone()),
                                (&this.smart_query, rule.query.clone()),
                                (&this.smart_artist, rule.artist.clone()),
                                (&this.smart_album, rule.album.clone()),
                                (&this.smart_plays, rule.minimum_plays.to_string()),
                            ] {
                                input.update(cx, |input, cx| input.set_value(value, window, cx));
                            }
                            this.content_revision += 1;
                            cx.notify();
                        })),
                );
            }
        } else if self.page.editable {
            let id = id.clone();
            row = row.child(
                Button::new("clean-remote-playlist")
                    .ghost()
                    .label(t("Remove duplicate songs"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(page) = this
                            .account
                            .page(&crate::account_sync::Key::Playlist(id.clone()))
                        else {
                            return;
                        };
                        if page.continuation.is_some() {
                            this.notice = Some(
                                t("Load the entire playlist before removing duplicates.").into(),
                            );
                            this.content_revision += 1;
                            cx.notify();
                            return;
                        }
                        let mut seen = std::collections::HashSet::new();
                        let tracks: Vec<_> = page
                            .tracks
                            .iter()
                            .filter(|t| !seen.insert(t.id.clone()))
                            .cloned()
                            .collect();
                        if !tracks.is_empty() {
                            this.account_edit(
                                Edit::RemoveMany {
                                    playlist: id.clone(),
                                    tracks,
                                },
                                cx,
                            );
                        }
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(row)
            .when(self.editing_smart.is_some(), |body| {
                body.child(self.smart_editor(cx))
            })
    }
    fn choose_playlist_cover(&mut self, id: String, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose a PNG or JPEG cover").into()),
        });
        cx.spawn(async move |owner, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else {
                return;
            };
            let Some(path) = paths.first().cloned() else {
                return;
            };
            let key = id.clone();
            let result = cx
                .background_spawn(async move {
                    let bytes =
                        raag::resource::read_limited(std::fs::File::open(path)?, 10 * 1024 * 1024)?;
                    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                        .with_guessed_format()?;
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(8192);
                    limits.max_image_height = Some(8192);
                    limits.max_alloc = Some(64 * 1024 * 1024);
                    reader.limits(limits);
                    let picture = reader.decode()?.thumbnail(1024, 1024);
                    let directory = data_dir().join("playlist-covers");
                    std::fs::create_dir_all(&directory)?;
                    let path = directory.join(format!(
                        "{}-{:016x}.png",
                        raag::cache::key(&key),
                        rand::random::<u64>()
                    ));
                    picture.save_with_format(&path, image::ImageFormat::Png)?;
                    Ok::<_, anyhow::Error>(path)
                })
                .await;
            let _ = owner.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        this.library.playlist_metadata.entry(id).or_default().cover = Some(path);
                        this.navigation_revision += 1;
                        this.content_revision += 1;
                        this.save();
                    }
                    Err(error) => this.notice = Some(error.to_string()),
                };
                this.content_revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn reorder_playlist(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let mut order: Vec<_> = self
            .playlist_entries(usize::MAX)
            .iter()
            .map(|e| {
                e.track
                    .id
                    .strip_prefix("VL")
                    .unwrap_or(&e.track.id)
                    .to_owned()
            })
            .collect();
        if let Some(a) = order.iter().position(|id| id == from)
            && let Some(b) = order.iter().position(|id| id == to)
        {
            let id = order.remove(a);
            order.insert(b, id);
            self.content_revision += 1;
            self.library.playlist_order = order;
            self.navigation_revision += 1;
            self.save();
            cx.notify();
        }
    }
    pub(super) fn reload_lyrics(&mut self) {
        let Some(track) = self.selected.clone() else {
            return;
        };
        self.lyrics_revision = self.lyrics_revision.wrapping_add(1);
        self.lyrics_id = format!(
            "{}:{:?}:{}",
            track.id, self.library.lyrics.source, self.lyrics_revision
        );
        self.lyrics_loading = true;
        self.lyrics_error = None;
        let _ = self.backend.lyrics.send((
            self.lyrics_id.clone(),
            track,
            self.library.lyrics.source,
            true,
        ));
        self.content_revision += 1;
    }
    pub(super) fn lyric_time(&self) -> f64 {
        self.position
            + self
                .selected
                .as_ref()
                .map_or(0., |t| self.library.lyrics.offset(&t.id))
    }
    pub(super) fn lyric_height(&self) -> Rems {
        rems(
            (7. + 2.
                * (u8::from(self.library.lyrics.translations)
                    + u8::from(self.library.lyrics.romanization)) as f32)
                * self.library.lyrics.text_scale,
        )
    }
    pub(super) fn lyric_label(&self, line: &LyricLine, active: bool, color: Hsla) -> Div {
        let time = self.lyric_time();
        let text = if self.library.lyrics.words && active && !line.words.is_empty() {
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .children(line.words.iter().enumerate().map(|(index, word)| {
                    let end = word
                        .end
                        .or_else(|| line.words.get(index + 1).map(|w| w.start))
                        .unwrap_or(f64::INFINITY);
                    div()
                        .text_color(color.opacity(if time >= word.start && time < end {
                            1.
                        } else if time >= end {
                            0.75
                        } else {
                            0.3
                        }))
                        .child(word.text.clone())
                }))
                .into_any_element()
        } else {
            div().child(line.text.clone()).into_any_element()
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .text_center()
            .text_size(rems(1.65 * self.library.lyrics.text_scale))
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .opacity(if active || line.seconds.is_none() {
                1.
            } else {
                0.45
            })
            .child(text)
            .when(self.library.lyrics.translations, |body| {
                body.when_some(line.translation.clone(), |body, text| {
                    body.child(
                        div()
                            .text_size(rems(0.95 * self.library.lyrics.text_scale))
                            .font_weight(FontWeight::NORMAL)
                            .opacity(0.8)
                            .child(text),
                    )
                })
            })
            .when(self.library.lyrics.romanization, |body| {
                body.when_some(line.romanization.clone(), |body, text| {
                    body.child(
                        div()
                            .text_size(rems(0.95 * self.library.lyrics.text_scale))
                            .font_weight(FontWeight::NORMAL)
                            .opacity(0.7)
                            .child(text),
                    )
                })
            })
    }
    fn refresh_lyrics_display(&mut self, cx: &mut Context<Self>) {
        self.lyrics_revision = self.lyrics_revision.wrapping_add(1);
        self.lyrics_geometry = None;
        self.content_revision += 1;
        self.right_content.update(cx, |_, cx| cx.notify());
        self.chrome[ChromePart::ImmersiveLyrics as usize].update(cx, |_, cx| cx.notify());
        cx.notify();
    }
    fn import_lyrics(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.selected.clone() else {
            return;
        };
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose LRC or TTML lyrics").into()),
        });
        cx.spawn(async move |owner, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else {
                return;
            };
            let Some(path) = paths.first().cloned() else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    let file = std::fs::File::open(&path)?;
                    let bytes = raag::resource::read_limited(file, raag::resource::LYRIC_BYTES)?;
                    let lines = raag::lyrics::parse_document(
                        &bytes,
                        path.extension().is_some_and(|e| e == "ttml" || e == "xml"),
                    )?;
                    anyhow::ensure!(
                        !lines.is_empty(),
                        t("The selected file contains no lyrics.")
                    );
                    let value = raag::lyrics::Lyrics::custom(lines);
                    let path = data_dir()
                        .join("lyric-overrides")
                        .join(format!("{}.json", raag::cache::key(&track.id)));
                    raag::cache::write_json(&path, &value)
                })
                .await;
            let _ = owner.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.reload_lyrics(),
                    Err(error) => this.notice = Some(error.to_string()),
                };
                this.refresh_lyrics_display(cx);
            });
        })
        .detach();
    }
    pub(super) fn lyrics_preferences(&self, cx: &mut Context<Self>) -> Div {
        let selected = self.selected.is_some();
        let offset = self
            .selected
            .as_ref()
            .map_or(self.library.lyrics.offset_ms, |t| {
                self.library
                    .lyrics
                    .track_offsets
                    .get(&t.id)
                    .copied()
                    .unwrap_or(self.library.lyrics.offset_ms)
            });
        div().flex().flex_col().gap_3()
            .child(Button::new("lyrics-source").label(tr!("Lyrics provider: {}", self.library.lyrics.source.label())).on_click(cx.listener(|this, _, _, cx| {
                this.library.lyrics.source = this.library.lyrics.source.next(); this.reload_lyrics(); this.save(); this.refresh_lyrics_display(cx);
            })))
            .child(Switch::new("word-lyrics").label(t("Highlight words and syllables")).checked(self.library.lyrics.words).on_change(cx.listener(|this, value, _, cx| { this.library.lyrics.words = *value; this.lyrics_revision += 1; this.save(); this.refresh_lyrics_display(cx); })))
            .child(Switch::new("translated-lyrics").label(t("Show available translations")).checked(self.library.lyrics.translations).on_change(cx.listener(|this, value, _, cx| { this.library.lyrics.translations = *value; this.lyrics_revision += 1; this.save(); this.refresh_lyrics_display(cx); })))
            .child(Switch::new("romanized-lyrics").label(t("Show available romanization")).checked(self.library.lyrics.romanization).on_change(cx.listener(|this, value, _, cx| { this.library.lyrics.romanization = *value; this.lyrics_revision += 1; this.save(); this.refresh_lyrics_display(cx); })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t("Word timing and translations appear only when supplied by the lyric source.")))
            .child(div().flex().flex_wrap().items_center().gap_2()
                .child(div().text_sm().child(tr!("Lyrics offset: {offset} ms")))
                .children([-100, 100].into_iter().map(|delta| Button::new(SharedString::from(format!("lyric-offset-{delta}"))).label(if delta < 0 { t("Earlier") } else { t("Later") }).on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(track) = &this.selected { let value = this.library.lyrics.track_offsets.entry(track.id.clone()).or_insert(this.library.lyrics.offset_ms); *value = (*value + delta).clamp(-30_000, 30_000); }
                    else { this.library.lyrics.offset_ms = (this.library.lyrics.offset_ms + delta).clamp(-30_000, 30_000); }
                    this.lyrics_active = None; this.lyrics_revision += 1; this.save(); this.refresh_lyrics_display(cx);
                }))))
                .child(Button::new("reset-lyric-offset").ghost().label(t("Reset offset")).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(track) = &this.selected { this.library.lyrics.track_offsets.remove(&track.id); } else { this.library.lyrics.offset_ms = 0; }
                    this.lyrics_active = None; this.save(); this.refresh_lyrics_display(cx);
                }))))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(if selected { t("Offset is saved for the current song. Positive values show lyrics earlier.") } else { t("Play a song to adjust its timing. This setting changes the default offset.") }))
            .child(Button::new("lyric-text-scale").label(tr!("Lyrics text size: {:.0}%", self.library.lyrics.text_scale * 100.)).on_click(cx.listener(|this, _, _, cx| { this.library.lyrics.text_scale = if this.library.lyrics.text_scale >= 1.75 { 0.75 } else { this.library.lyrics.text_scale + 0.25 }; this.lyrics_active = None; this.save(); this.refresh_lyrics_display(cx); })))
            .child(div().flex().flex_wrap().gap_2()
                .child(Button::new("reload-lyrics").label(t("Reload lyrics")).disabled(!selected).on_click(cx.listener(|this, _, _, cx| { this.reload_lyrics(); this.refresh_lyrics_display(cx); })))
                .child(Button::new("import-lyrics").label(t("Import corrected lyrics…")).disabled(!selected).on_click(cx.listener(|this, _, _, cx| this.import_lyrics(cx))))
                .child(Button::new("clear-lyric-override").ghost().label(t("Use provider lyrics")).disabled(!selected).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(track) = &this.selected { let path = data_dir().join("lyric-overrides").join(format!("{}.json", raag::cache::key(&track.id))); if let Err(error) = std::fs::remove_file(path) && error.kind() != std::io::ErrorKind::NotFound { this.notice = Some(error.to_string()); this.refresh_lyrics_display(cx); return; } }
                    this.reload_lyrics(); this.refresh_lyrics_display(cx);
                }))))
    }
    pub(super) fn move_track_to(
        &mut self,
        dragged: &TrackDrag,
        target: usize,
        screen: &Screen,
        cx: &mut Context<Self>,
    ) {
        if dragged.screen != *screen || target == dragged.index {
            return;
        }
        if *screen == Screen::Queue {
            self.queue.move_to(dragged.index, target);
            self.invalidate_collection();
            self.save();
            self.refresh_visible();
            cx.notify();
            return;
        }
        let Screen::Collection(id, _, local) = screen else {
            return;
        };
        if *local {
            if let Some(playlist) = self
                .library
                .playlists
                .iter_mut()
                .find(|p| p.id == *id && p.smart.is_none())
                && dragged.index < playlist.tracks.len()
                && target < playlist.tracks.len()
            {
                let track = playlist.tracks.remove(dragged.index);
                playlist.tracks.insert(target, track);
                self.save();
                self.refresh_visible();
                self.navigation_revision += 1;
                cx.notify();
            }
        } else if self.page.editable {
            let Some(page) = self
                .account
                .page(&crate::account_sync::Key::Playlist(id.clone()))
            else {
                return;
            };
            let Some(entry) = dragged.track.set_video_id.clone() else {
                return;
            };
            let before = page
                .tracks
                .get(if dragged.index < target {
                    target + 1
                } else {
                    target
                })
                .and_then(|t| t.set_video_id.clone());
            if before.is_none() && page.continuation.is_some() {
                self.notice =
                    Some(t("Load the remaining playlist before moving a song to the end.").into());
                cx.notify();
                return;
            }
            self.account_edit(
                Edit::Move {
                    playlist: id.clone(),
                    entry,
                    before,
                },
                cx,
            );
        }
    }
}
