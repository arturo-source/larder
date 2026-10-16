//! The "connect to an endpoint" page: saved profiles plus the connection form.

use std::cell::RefCell;

use adw::prelude::*;
use gtk::gio;

use crate::config::{Config, Profile};

pub struct ConnectPage {
    pub root: adw::ToolbarView,
    profiles_group: adw::PreferencesGroup,
    profile_rows: RefCell<Vec<adw::ActionRow>>,
    name: adw::EntryRow,
    endpoint: adw::EntryRow,
    region: adw::EntryRow,
    access_key: adw::EntryRow,
    secret_key: adw::PasswordEntryRow,
    path_style: adw::SwitchRow,
    pub connect_button: gtk::Button,
    pub save_button: gtk::Button,
    spinner: gtk::Spinner,
}

impl ConnectPage {
    pub fn new(menu: &gio::MenuModel) -> Self {
        let header = adw::HeaderBar::new();
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(menu)
                .build(),
        );

        let profiles_group = adw::PreferencesGroup::builder()
            .title("Saved Profiles")
            .visible(false)
            .build();

        let name = adw::EntryRow::builder().title("Profile Name").build();
        let endpoint = adw::EntryRow::builder().title("Endpoint URL").build();
        let region = adw::EntryRow::builder().title("Region").build();
        let access_key = adw::EntryRow::builder().title("Access Key").build();
        let secret_key = adw::PasswordEntryRow::builder().title("Secret Key").build();
        let path_style = adw::SwitchRow::builder()
            .title("Path-Style Addressing")
            .subtitle("Needed by most self-hosted servers such as RustFS or MinIO")
            .build();
        let form = adw::PreferencesGroup::builder()
            .title("Connection")
            .description("Leave the endpoint empty to use Amazon S3")
            .build();
        for row in [
            name.upcast_ref::<gtk::Widget>(),
            endpoint.upcast_ref(),
            region.upcast_ref(),
            access_key.upcast_ref(),
        ] {
            form.add(row);
        }
        form.add(&secret_key);
        form.add(&path_style);

        let connect_button = gtk::Button::builder()
            .label("Connect")
            .css_classes(["suggested-action", "pill"])
            .build();
        let save_button = gtk::Button::builder()
            .label("Save Profile")
            .css_classes(["pill"])
            .build();
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .visible(false)
            .build();
        let buttons = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(6)
            .build();
        buttons.append(&save_button);
        buttons.append(&connect_button);
        buttons.append(&spinner);

        let note = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label", "caption"])
            .build();
        if let Some(path) = Config::path() {
            note.set_label(&format!(
                "Profiles, including secret keys, are stored in plain text in {}",
                path.display()
            ));
        }

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        content.append(&profiles_group);
        content.append(&form);
        content.append(&buttons);
        content.append(&note);

        let status = adw::StatusPage::builder()
            .icon_name("network-server-symbolic")
            .title("Connect to Storage")
            .description("Browse buckets on Amazon S3, RustFS, MinIO or any S3-compatible server")
            .child(
                &adw::Clamp::builder()
                    .maximum_size(520)
                    .child(&content)
                    .build(),
            )
            .build();

        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&status));

        // Enter in any field connects.
        for entry in [&name, &endpoint, &region, &access_key] {
            entry.connect_entry_activated({
                let b = connect_button.clone();
                move |_| b.emit_clicked()
            });
        }
        secret_key.connect_entry_activated({
            let b = connect_button.clone();
            move |_| b.emit_clicked()
        });

        Self {
            root,
            profiles_group,
            profile_rows: RefCell::default(),
            name,
            endpoint,
            region,
            access_key,
            secret_key,
            path_style,
            connect_button,
            save_button,
            spinner,
        }
    }

    pub fn set_form(&self, p: &Profile) {
        self.name.set_text(&p.name);
        self.endpoint.set_text(&p.endpoint);
        self.region.set_text(&p.region);
        self.access_key.set_text(&p.access_key);
        self.secret_key.set_text(&p.secret_key);
        self.path_style.set_active(p.path_style);
    }

    pub fn form(&self) -> Profile {
        Profile {
            name: self.name.text().trim().to_string(),
            endpoint: self.endpoint.text().trim().to_string(),
            region: self.region.text().trim().to_string(),
            access_key: self.access_key.text().trim().to_string(),
            secret_key: self.secret_key.text().to_string(),
            path_style: self.path_style.is_active(),
        }
    }

    pub fn set_busy(&self, busy: bool) {
        self.spinner.set_visible(busy);
        self.connect_button.set_sensitive(!busy);
    }

    /// Rebuilds the saved-profile rows. `on_select` loads a profile into the form, `on_delete` removes it.
    pub fn set_profiles(
        &self,
        profiles: &[Profile],
        on_select: impl Fn(&Profile) + Clone + 'static,
        on_delete: impl Fn(&str) + Clone + 'static,
    ) {
        for row in self.profile_rows.borrow_mut().drain(..) {
            self.profiles_group.remove(&row);
        }
        for p in profiles {
            let row = adw::ActionRow::builder()
                .title(&p.name)
                .subtitle(display_endpoint(&p.endpoint))
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Delete Profile")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            row.add_suffix(&delete);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let profile = p.clone();
            let select = on_select.clone();
            row.connect_activated(move |_| select(&profile));
            let name = p.name.clone();
            let delete_cb = on_delete.clone();
            delete.connect_clicked(move |_| delete_cb(&name));
            self.profiles_group.add(&row);
            self.profile_rows.borrow_mut().push(row);
        }
        self.profiles_group.set_visible(!profiles.is_empty());
    }
}

pub fn display_endpoint(endpoint: &str) -> String {
    if endpoint.is_empty() {
        "Amazon S3".into()
    } else {
        endpoint.to_string()
    }
}
