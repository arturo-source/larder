//! Side panel with metadata and a preview of the selected object.

use std::cell::RefCell;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::s3::{Entry, ObjectDetails, Preview, human_size};

pub struct DetailsPane {
    pub root: gtk::ScrolledWindow,
    icon: gtk::Image,
    name: gtk::Label,
    summary: gtk::Label,
    props: Vec<(&'static str, adw::ActionRow)>,
    meta_group: adw::PreferencesGroup,
    meta_rows: RefCell<Vec<adw::ActionRow>>,
    preview_stack: gtk::Stack,
    picture: gtk::Picture,
    text: gtk::TextView,
    preview_note: gtk::Label,
    pub download_button: gtk::Button,
    pub share_button: gtk::Button,
}

const PROPS: [(&str, &str); 8] = [
    ("type", "Type"),
    ("size", "Size"),
    ("modified", "Modified"),
    ("etag", "ETag"),
    ("class", "Storage Class"),
    ("version", "Version"),
    ("cache", "Cache-Control"),
    ("encoding", "Content-Encoding"),
];

impl DetailsPane {
    pub fn new() -> Self {
        let icon = gtk::Image::builder()
            .pixel_size(64)
            .css_classes(["dim-label"])
            .build();
        let name = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .css_classes(["title-3"])
            .build();
        let summary = gtk::Label::builder().css_classes(["dim-label"]).build();

        let download_button = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("folder-download-symbolic")
                    .label("Download")
                    .build(),
            )
            .css_classes(["pill", "suggested-action"])
            .build();
        let share_button = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("emblem-shared-symbolic")
                    .label("Share Link")
                    .build(),
            )
            .css_classes(["pill"])
            .build();
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::Center)
            .homogeneous(true)
            .build();
        buttons.append(&download_button);
        buttons.append(&share_button);

        let props_group = adw::PreferencesGroup::builder().title("Properties").build();
        let props = PROPS
            .iter()
            .map(|(id, title)| {
                let row = adw::ActionRow::builder()
                    .title(*title)
                    .subtitle_selectable(true)
                    .css_classes(["property"])
                    .build();
                props_group.add(&row);
                (*id, row)
            })
            .collect();
        let meta_group = adw::PreferencesGroup::builder()
            .title("Custom Metadata")
            .visible(false)
            .build();

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .height_request(220)
            .build();
        let text = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .css_classes(["preview-text"])
            .build();
        let text_scroll = gtk::ScrolledWindow::builder()
            .child(&text)
            .height_request(260)
            .build();
        let preview_note = gtk::Label::builder()
            .wrap(true)
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build();
        let preview_stack = gtk::Stack::new();
        preview_stack.add_named(&picture, Some("image"));
        preview_stack.add_named(&text_scroll, Some("text"));
        preview_stack.add_named(&preview_note, Some("note"));
        preview_stack.add_named(
            &adw::Spinner::builder()
                .height_request(32)
                .margin_top(24)
                .margin_bottom(24)
                .build(),
            Some("loading"),
        );
        let preview_frame = gtk::Frame::builder()
            .child(&preview_stack)
            .css_classes(["preview-frame"])
            .build();
        let preview_group = adw::PreferencesGroup::builder().title("Preview").build();
        preview_group.add(&preview_frame);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .build();
        let head = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        head.append(&icon);
        head.append(&name);
        head.append(&summary);
        content.append(&head);
        content.append(&buttons);
        content.append(&preview_group);
        content.append(&props_group);
        content.append(&meta_group);

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .width_request(300)
            .build();

        Self {
            root,
            icon,
            name,
            summary,
            props,
            meta_group,
            meta_rows: RefCell::default(),
            preview_stack,
            picture,
            text,
            preview_note,
            download_button,
            share_button,
        }
    }

    /// Shows what we already know from the listing while the details load.
    pub fn show_loading(&self, e: &Entry) {
        let (content_type, _) = gio::content_type_guess(Some(e.name.as_str()), None);
        self.icon
            .set_from_gicon(&gio::content_type_get_icon(&content_type));
        self.name.set_label(base_name(&e.key));
        self.summary.set_label(&human_size(e.size));
        for (_, row) in &self.props {
            row.set_visible(false);
        }
        self.set_metadata(&[]);
        self.preview_stack.set_visible_child_name("loading");
    }

    pub fn show(&self, d: &ObjectDetails, preview: &Result<Preview, String>) {
        let ct = d.content_type.clone().unwrap_or_default();
        self.summary.set_label(&if ct.is_empty() {
            human_size(d.size)
        } else {
            format!("{} · {ct}", human_size(d.size))
        });
        let value = |id: &str| -> Option<String> {
            match id {
                "type" => d.content_type.clone(),
                "size" => Some(format!("{} ({} bytes)", human_size(d.size), d.size)),
                "modified" => d.modified.clone(),
                "etag" => d.etag.as_ref().map(|e| e.trim_matches('"').to_string()),
                "class" => d.storage_class.clone(),
                "version" => d.version_id.clone().filter(|v| v != "null"),
                "cache" => d.cache_control.clone(),
                "encoding" => d.content_encoding.clone(),
                _ => None,
            }
        };
        for (id, row) in &self.props {
            match value(id) {
                Some(v) if !v.is_empty() => {
                    row.set_subtitle(&v);
                    row.set_visible(true);
                }
                _ => row.set_visible(false),
            }
        }
        let meta: Vec<(String, String)> = d
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.set_metadata(&meta);

        match preview {
            Ok(Preview::Image(bytes)) => {
                match gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
                    Ok(texture) => {
                        self.picture.set_paintable(Some(&texture));
                        self.preview_stack.set_visible_child_name("image");
                    }
                    Err(_) => self.note("This image format can't be previewed"),
                }
            }
            Ok(Preview::Text { text, truncated }) if text.is_empty() && !truncated => {
                self.note("Empty file")
            }
            Ok(Preview::Text { text, truncated }) => {
                let mut shown = text.clone();
                if *truncated {
                    shown.push_str("\n\n… (only the first 64 KiB are shown)");
                }
                self.text.buffer().set_text(&shown);
                self.preview_stack.set_visible_child_name("text");
            }
            Ok(Preview::None(why)) => self.note(why),
            Err(e) => self.note(&format!("Preview failed: {e}")),
        }
    }

    pub fn show_error(&self, message: &str) {
        self.note(message);
    }

    fn note(&self, text: &str) {
        self.preview_note.set_label(text);
        self.preview_stack.set_visible_child_name("note");
    }

    fn set_metadata(&self, meta: &[(String, String)]) {
        for row in self.meta_rows.borrow_mut().drain(..) {
            self.meta_group.remove(&row);
        }
        for (k, v) in meta {
            let row = adw::ActionRow::builder()
                .title(k)
                .subtitle(v)
                .subtitle_selectable(true)
                .css_classes(["property"])
                .build();
            self.meta_group.add(&row);
            self.meta_rows.borrow_mut().push(row);
        }
        self.meta_group.set_visible(!meta.is_empty());
    }
}

pub fn base_name(key: &str) -> &str {
    let k = key.trim_end_matches('/');
    k.rsplit('/').next().unwrap_or(k)
}
