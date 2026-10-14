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
    fn set_language(&mut self, code: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.library.language = code.into();
        raag::i18n::set_language(code);
        gpui_kit::component::set_locale(code);
        if let Some(system) = &self.system {
            system.refresh_language();
        }
        for (input, placeholder) in [
            (&self.query, "Search songs, artists, albums…"),
            (&self.playlist_name, "Playlist name"),
            (&self.organization_name, "Folder name"),
            (&self.smart_name, "Smart playlist name"),
            (&self.smart_query, "Song title, artist or album"),
            (&self.smart_artist, "Any artist"),
            (&self.smart_album, "Any album"),
            (&self.browse_query, "Filter this collection…"),
            (&self.ytdlp_path, "Automatic (yt-dlp on PATH or bundled)"),
        ] {
            input.update(cx, |input, cx| {
                input.set_placeholder(t(placeholder), window, cx)
            });
        }
        self.apply_appearance(cx);
        self.save();
    }
    pub(super) fn localization_settings(&self, cx: &mut Context<Self>) -> Div {
        let owner = cx.weak_entity();
        let selected = self.library.language.clone();
        let label = match selected.as_str() {
            "hi" => "हिन्दी",
            "es" => "Español",
            _ => "English",
        };
        let language = Button::new("language")
            .accessibility_label(t("Language"))
            .label(label)
            .dropdown_menu(move |mut menu, _, _| {
                for (code, label) in [("en", "English"), ("hi", "हिन्दी"), ("es", "Español")]
                {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .checked(selected == code)
                            .on_click(move |_, window, cx| {
                                let _ = owner
                                    .update(cx, |this, cx| this.set_language(code, window, cx));
                            }),
                    );
                }
                menu
            });
        let owner = cx.weak_entity();
        let selected = self.library.ui_scale;
        let scale = Button::new("interface-scale")
            .accessibility_label(t("Interface scale"))
            .label(format!("{:.0}%", selected * 100.))
            .dropdown_menu(move |mut menu, _, _| {
                for scale in [0.75f32, 0.9, 1., 1.1, 1.25, 1.5, 1.75] {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{:.0}%", scale * 100.))
                            .checked((selected - scale).abs() < 0.01)
                            .on_click(move |_, window, cx| {
                                let _ = owner
                                    .update(cx, |this, cx| this.set_ui_scale(scale, window, cx));
                            }),
                    );
                }
                menu
            });
        self.settings_section(
            t("Language and size"),
            t("Interface language and display scale"),
            cx,
        )
        .child(self.setting_row(t("Language"), language, cx))
        .child(self.setting_row(t("Interface scale"), scale, cx))
    }

    pub(super) fn organization_controls(&self, cx: &mut Context<Self>) -> Div {
        let parent = self
            .library
            .active_folder
            .as_ref()
            .and_then(|id| self.library.folders.iter().find(|f| &f.id == id))
            .and_then(|f| f.parent.clone());
        let mut body = div().flex().flex_col().gap_3().flex_shrink_0();
        let mut folders = div().flex().flex_wrap().items_center().gap_2().when(
            self.library.active_folder.is_some(),
            |row| {
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
            },
        );
        folders = folders.child(
            div()
                .id("root-folder-drop")
                .px_3()
                .py_2()
                .rounded_lg()
                .text_sm()
                .child(
                    Button::new("all-folders")
                        .ghost()
                        .label(t("All playlists"))
                        .selected(self.library.active_folder.is_none())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.library.active_folder = None;
                            this.folder_editor = false;
                            this.navigation_revision += 1;
                            this.content_revision += 1;
                            this.save();
                            cx.notify();
                        })),
                )
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
        body = body.when(!self.library.folders.is_empty(), |body| body.child(folders)).when(self.folder_editor, |body| body.child(
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
                        .primary().label(if self.folder_rename { t("Rename folder") } else { t("New folder") })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let name = this.organization_name.read(cx).value().to_string();
                            if this.folder_rename {
                                let name = name.trim();
                                if name.is_empty() || name.chars().count() > 100 { this.notice = Some(t("Enter a folder name between 1 and 100 characters.").into()); this.content_revision += 1; cx.notify(); return; }
                                if let Some(folder) = this.library.folders.iter_mut().find(|folder| Some(&folder.id) == this.library.active_folder.as_ref()) { folder.name = name.to_owned(); }
                            } else if let Err(error) = this.library.create_folder(&name, this.library.active_folder.clone()) {
                                this.notice = Some(error.to_string()); this.content_revision += 1; cx.notify(); return;
                            }
                            this.folder_editor = false;
                            this.navigation_revision += 1;
                            this.save();
                            this.content_revision += 1;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("cancel-folder-editor").ghost().label(t("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| { this.folder_editor = false; this.content_revision += 1; cx.notify(); })),
                ),
        ));
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
        div().when(self.editing_smart.is_some(), |body| {
            body.child(self.smart_editor(cx))
        })
    }
    pub(super) fn collection_actions(
        &self,
        owner: WeakEntity<Self>,
    ) -> impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + use<> {
        let collection = match &self.screen {
            Screen::Collection(id, name, local) => Some((id.clone(), name.clone(), *local)),
            _ => None,
        };
        let key = collection
            .as_ref()
            .map(|(id, _, _)| id.strip_prefix("VL").unwrap_or(id));
        let folders = self.library.folders.clone();
        let current = key
            .and_then(|id| self.library.playlist_metadata.get(id))
            .and_then(|meta| meta.folder.clone());
        let rule = collection
            .as_ref()
            .and_then(|(id, _, _)| {
                self.library
                    .playlists
                    .iter()
                    .find(|playlist| &playlist.id == id)
            })
            .and_then(|playlist| playlist.smart.clone());
        let editable = self.page.editable;
        move |mut menu, window, cx| {
            let Some((id, name, local)) = collection else {
                return menu;
            };
            let key = id.strip_prefix("VL").unwrap_or(&id).to_owned();
            if !local {
                let link = if id.starts_with("MPRE") {
                    raag::links::Link::Album(id.clone())
                } else if id.starts_with("UC") {
                    raag::links::Link::Artist(id.clone())
                } else {
                    raag::links::Link::Playlist(key.clone())
                };
                menu = menu.item(PopupMenuItem::new(t("Copy music link")).on_click(
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(link.public_url()));
                    },
                ));
            }
            if !local && (id.starts_with("MPRE") || id.starts_with("UC")) {
                return menu;
            }
            let choices: Vec<_> = std::iter::once((None, t("None").to_owned()))
                .chain(
                    folders
                        .iter()
                        .map(|folder| (Some(folder.id.clone()), folder.name.clone())),
                )
                .collect();
            let folder_owner = owner.clone();
            let folder_key = key.clone();
            menu =
                menu.separator()
                    .submenu(t("Move to folder"), window, cx, move |mut menu, _, _| {
                        for (folder, name) in &choices {
                            let owner = folder_owner.clone();
                            let key = folder_key.clone();
                            let folder = folder.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(folder == current)
                                    .on_click(move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.library
                                                .playlist_metadata
                                                .entry(key.clone())
                                                .or_default()
                                                .folder = folder.clone();
                                            this.navigation_revision += 1;
                                            this.content_revision += 1;
                                            this.save();
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    });
            for (label, command) in [
                (t("Choose cover on this device…"), 0),
                (t("Reset cover"), 1),
            ] {
                let owner = owner.clone();
                let key = key.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if command == 0 {
                            this.choose_playlist_cover(key.clone(), cx);
                        } else {
                            this.library
                                .playlist_metadata
                                .entry(key.clone())
                                .or_default()
                                .cover = None;
                            this.navigation_revision += 1;
                            this.content_revision += 1;
                            this.save();
                            cx.notify();
                        }
                    });
                }));
            }
            if local || editable {
                let owner = owner.clone();
                menu = menu.item(PopupMenuItem::new(t("Remove duplicate songs")).on_click(
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| this.deduplicate_visible_playlist(cx));
                    },
                ));
            }
            if local && let Some(rule) = rule {
                let owner = owner.clone();
                let id = id.clone();
                let name = name.clone();
                menu = menu.item(PopupMenuItem::new(t("Edit smart rules")).on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.editing_smart = Some(id.clone());
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
                        });
                    },
                ));
            }
            menu
        }
    }
    fn deduplicate_visible_playlist(&mut self, cx: &mut Context<Self>) {
        let Screen::Collection(id, _, local) = &self.screen else {
            return;
        };
        if *local {
            let count = self.library.deduplicate_playlist(id);
            self.notice = Some(tr!("Removed {count} duplicate songs"));
            self.save();
            self.refresh_visible();
            self.navigation_revision += 1;
        } else if !self.account.busy_edits() && self.pending_edits.is_empty() {
            let Some(page) = self
                .account
                .page(&crate::account_sync::Key::Playlist(id.clone()))
            else {
                return;
            };
            if page.continuation.is_some() {
                self.notice =
                    Some(t("Load the entire playlist before removing duplicates.").into());
            } else {
                let mut seen = std::collections::HashSet::new();
                let tracks: Vec<_> = page
                    .tracks
                    .iter()
                    .filter(|track| !seen.insert(track.id.clone()))
                    .cloned()
                    .collect();
                for chunk in tracks.chunks(100) {
                    self.pending_edits.push_back(Edit::RemoveMany {
                        playlist: id.clone(),
                        tracks: chunk.to_vec(),
                    });
                }
            }
        }
        self.content_revision += 1;
        cx.notify();
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
        let owner = cx.weak_entity();
        let mut body = div().flex().flex_col().gap_1()
            .child(self.setting_row(t("Lyrics provider"), Button::new("lyrics-source").accessibility_label(t("Lyrics provider")).label(self.library.lyrics.source.label())
                .dropdown_menu(move |mut menu, _, _| {
                    use raag::lyrics::Source;
                    for value in Source::ALL {
                        let owner = owner.clone();
                        menu = menu.item(PopupMenuItem::new(value.label()).on_click(move |_, _, cx| { let _ = owner.update(cx, |this, cx| {
                            this.library.lyrics.source = value; this.reload_lyrics(); this.save(); this.refresh_lyrics_display(cx);
                        }); }));
                    } menu
                }), cx))
            .child(self.setting_row(t("Highlight words and syllables"), Switch::new("word-lyrics").accessibility_label(t("Highlight words and syllables")).checked(self.library.lyrics.words).on_change(cx.listener(|this, value, _, cx| { this.library.lyrics.words = *value; this.save(); this.refresh_lyrics_display(cx); })), cx))
            .child(self.setting_row(t("Show available translations"), Switch::new("translated-lyrics").accessibility_label(t("Show available translations")).checked(self.library.lyrics.translations).on_change(cx.listener(|this, value, _, cx| { this.library.lyrics.translations = *value; this.save(); this.refresh_lyrics_display(cx); })), cx))
            .child(self.setting_row(t("Show available romanization"), Switch::new("romanized-lyrics").accessibility_label(t("Show available romanization")).checked(self.library.lyrics.romanization).on_change(cx.listener(|this, value, _, cx| { this.library.lyrics.romanization = *value; this.save(); this.refresh_lyrics_display(cx); })), cx))
            .child(div().text_sm().py_3().text_color(cx.theme().muted_foreground).child(t("Word timing and translations appear only when supplied by the lyric source.")));
        let owner = cx.weak_entity();
        body = body.child(
            self.setting_row(
                t("Lyrics text size"),
                Button::new("lyric-text-scale")
                    .accessibility_label(t("Lyrics text size"))
                    .label(format!("{:.0}%", self.library.lyrics.text_scale * 100.))
                    .dropdown_menu(move |mut menu, _, _| {
                        for scale in [0.75f32, 1., 1.25, 1.5, 1.75] {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{:.0}%", scale * 100.)).on_click(
                                    move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.library.lyrics.text_scale = scale;
                                            this.lyrics_active = None;
                                            this.save();
                                            this.refresh_lyrics_display(cx);
                                        });
                                    },
                                ),
                            );
                        }
                        menu
                    }),
                cx,
            ),
        );
        body.child(
            div().pt_5().pb_2().font_weight(FontWeight::SEMIBOLD).child(
                self.selected
                    .as_ref()
                    .map_or(t("Default timing").to_owned(), |track| track.title.clone()),
            ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(div().text_sm().child(tr!("Lyrics offset: {offset} ms")))
                .children([-100, 100].into_iter().map(|delta| {
                    Button::new(SharedString::from(format!("lyric-offset-{delta}")))
                        .label(if delta < 0 { t("Earlier") } else { t("Later") })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(track) = &this.selected {
                                let value = this
                                    .library
                                    .lyrics
                                    .track_offsets
                                    .entry(track.id.clone())
                                    .or_insert(this.library.lyrics.offset_ms);
                                *value = (*value + delta).clamp(-30_000, 30_000);
                            } else {
                                this.library.lyrics.offset_ms =
                                    (this.library.lyrics.offset_ms + delta).clamp(-30_000, 30_000);
                            }
                            this.lyrics_active = None;
                            this.lyrics_revision += 1;
                            this.save();
                            this.refresh_lyrics_display(cx);
                        }))
                }))
                .child(
                    Button::new("reset-lyric-offset")
                        .ghost()
                        .label(t("Reset offset"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(track) = &this.selected {
                                this.library.lyrics.track_offsets.remove(&track.id);
                            } else {
                                this.library.lyrics.offset_ms = 0;
                            }
                            this.lyrics_active = None;
                            this.save();
                            this.refresh_lyrics_display(cx);
                        })),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(if selected {
                    t("Offset is saved for the current song. Positive values show lyrics earlier.")
                } else {
                    t("Play a song to adjust its timing. This setting changes the default offset.")
                }),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("reload-lyrics")
                        .label(t("Reload lyrics"))
                        .disabled(!selected)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reload_lyrics();
                            this.refresh_lyrics_display(cx);
                        })),
                )
                .child(
                    Button::new("import-lyrics")
                        .label(t("Import corrected lyrics…"))
                        .disabled(!selected)
                        .on_click(cx.listener(|this, _, _, cx| this.import_lyrics(cx))),
                )
                .child(
                    Button::new("clear-lyric-override")
                        .ghost()
                        .label(t("Use provider lyrics"))
                        .disabled(!selected)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(track) = &this.selected {
                                let path = data_dir()
                                    .join("lyric-overrides")
                                    .join(format!("{}.json", raag::cache::key(&track.id)));
                                if let Err(error) = std::fs::remove_file(path)
                                    && error.kind() != std::io::ErrorKind::NotFound
                                {
                                    this.notice = Some(error.to_string());
                                    this.refresh_lyrics_display(cx);
                                    return;
                                }
                            }
                            this.reload_lyrics();
                            this.refresh_lyrics_display(cx);
                        })),
                ),
        )
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
