use super::*;
impl Raag {
    pub(super) fn artist_link(&self, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        if track.artist.is_empty() || track.kind == ItemKind::Local {
            return div()
                .truncate()
                .child(track.artist.clone())
                .into_any_element();
        }
        let artists = if track.artists.is_empty() {
            track
                .artist
                .split(" & ")
                .flat_map(|name| name.split(", "))
                .filter(|name| !name.trim().is_empty())
                .map(|name| {
                    let mut artist = raag::ArtistRef::default();
                    artist.name = name.trim().to_owned();
                    artist
                })
                .collect()
        } else {
            track.artists.clone()
        };
        let mut row = div().flex().items_center().min_w_0().gap_1().text_left();
        for (index, artist) in artists.into_iter().enumerate() {
            if index > 0 {
                row = row.child(div().flex_shrink_0().child(","));
            }
            let name = artist.name.clone();
            row = row.child(
                content_button(SharedString::from(format!(
                    "artist-link-{}-{index}",
                    track.id
                )))
                .justify_start()
                .min_w_0()
                .cursor_pointer()
                .hover(|div| div.underline())
                .accessibility_label(tr!("Open artist {name}"))
                .child(div().min_w_0().truncate().child(name))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.immersive {
                        this.exit_immersive(window, cx);
                    }
                    this.open_artist(artist.clone(), cx);
                    cx.stop_propagation();
                })),
            );
        }
        row.into_any_element()
    }
    pub(super) fn artist_name(&self, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        if track.kind != ItemKind::Artist {
            return div()
                .truncate()
                .child(track.title.clone())
                .into_any_element();
        }
        let owned = track.clone();
        content_button(SharedString::from(format!("artist-title-{}", track.id)))
            .min_w_0()
            .justify_start()
            .cursor_pointer()
            .hover(|div| div.underline())
            .accessibility_label(tr!("Open artist {}", track.title))
            .child(div().truncate().child(track.title.clone()))
            .on_click(cx.listener(move |this, _, _, cx| this.open_track(owned.clone(), cx)))
            .into_any_element()
    }
    fn open_artist(&mut self, artist: raag::ArtistRef, cx: &mut Context<Self>) {
        if !artist.id.is_empty() {
            let mut track = Track::song(artist.id, artist.name, "");
            track.kind = ItemKind::Artist;
            self.open_track(track, cx);
        } else {
            // Old saved tracks and editorial seeds may have names without browse endpoints.
            let name = artist
                .name
                .split(" & ")
                .next()
                .unwrap_or(&artist.name)
                .split(", ")
                .next()
                .unwrap_or(&artist.name)
                .to_owned();
            self.navigate(
                Screen::Collection(format!("artist:{name}"), name.clone(), false),
                cx,
            );
            let mut seed = Track::song(format!("artist:{name}"), &name, "");
            seed.kind = ItemKind::Artist;
            self.collection_seed = Some(seed);
            self.request(|id| Request::Artist(id, name), cx);
        }
    }
}
