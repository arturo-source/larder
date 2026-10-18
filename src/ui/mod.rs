//! The GTK4 / libadwaita user interface.

mod connect;
mod details;
mod dialogs;
mod objects;
mod properties;
mod transfers;

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use adw::prelude::*;
use aws_sdk_s3::Client;
use gtk::{gdk, gio, glib};

use crate::config::{Config, Profile};
use crate::runtime::bg;
use crate::s3::{self, BucketInfo, Entry, Progress, human_size};
use details::base_name;

pub const APP_ID: &str = "io.github.arturo_source.Larder";
pub const APP_NAME: &str = "Larder";

pub fn set_accels(app: &adw::Application) {
    let accels: [(&str, &[&str]); 11] = [
        ("win.refresh", &["F5", "<Control>r"]),
        ("win.up", &["<Alt>Up", "<Alt>Left"]),
        ("win.upload", &["<Control>u"]),
        ("win.new-folder", &["<Control><Shift>n"]),
        ("win.search", &["<Control>f"]),
        ("win.delete", &["Delete"]),
        ("win.rename", &["F2"]),
        ("win.download", &["<Control>s"]),
        ("win.share", &["<Control>l"]),
        ("window.close", &["<Control>w"]),
        ("app.quit", &["<Control>q"]),
    ];
    for (action, keys) in accels {
        app.set_accels_for_action(action, keys);
    }
}

#[derive(Clone, Copy)]
enum Refresh {
    None,
    Listing,
    Buckets,
}

#[derive(Default)]
struct State {
    config: Config,
    client: Option<Client>,
    profile: Option<Profile>,
    buckets: Vec<BucketInfo>,
    bucket: Option<String>,
    prefix: String,
    recursive: bool,
    listing_generation: u64,
    details_key: Option<String>,
    pending: usize,
}

pub struct Win {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    connect: connect::ConnectPage,
    split: adw::NavigationSplitView,
    bucket_list: gtk::ListBox,
    bucket_menu: gtk::PopoverMenu,
    profile_label: gtk::Label,
    title: adw::WindowTitle,
    busy: gtk::Spinner,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    objects: objects::ObjectsView,
    details: details::DetailsPane,
    details_split: adw::OverlaySplitView,
    selection_bar: gtk::ActionBar,
    selection_label: gtk::Label,
    transfers: Rc<transfers::TransfersBar>,
    state: RefCell<State>,
}

fn menu(sections: &[&[(&str, &str)]]) -> gio::Menu {
    let menu = gio::Menu::new();
    for items in sections {
        let section = gio::Menu::new();
        for (label, action) in *items {
            section.append(Some(label), Some(action));
        }
        menu.append_section(None, &section);
    }
    menu
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
}

fn labeled_button(icon: &str, label: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name(icon)
                .label(label)
                .can_shrink(true)
                .build(),
        )
        .tooltip_text(label)
        .action_name(action)
        .build()
}

impl Win {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(1180)
            .default_height(760)
            .width_request(480)
            .height_request(420)
            .build();

        let about = format!("About {APP_NAME}");
        let main_menu = menu(&[
            &[
                ("New Bucket…", "win.new-bucket"),
                ("Refresh", "win.refresh"),
            ],
            &[("Show Contents of Subfolders", "win.flat")],
            &[("Disconnect", "win.disconnect")],
            &[
                ("Keyboard Shortcuts", "win.show-help-overlay"),
                (&about, "app.about"),
            ],
        ]);
        let connect_menu = menu(&[&[(&about, "app.about")]]);
        let context_menu = menu(&[
            &[
                ("Open", "win.open"),
                ("Download…", "win.download"),
                ("Share Link…", "win.share"),
            ],
            &[
                ("Rename…", "win.rename"),
                ("Edit Properties…", "win.edit-properties"),
                ("Copy Key", "win.copy-key"),
            ],
            &[("Delete…", "win.delete")],
        ]);
        let upload_menu = menu(&[&[("Upload Folder…", "win.upload-folder")]]);

        let connect = connect::ConnectPage::new(connect_menu.upcast_ref());

        // ---- Sidebar: buckets
        let bucket_list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let bucket_menu = gtk::PopoverMenu::builder()
            .has_arrow(false)
            .halign(gtk::Align::Start)
            .build();
        // The popover can't be parented to the list itself: ListBox::remove_all() would try to remove it.
        let bucket_list_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        bucket_list_box.append(&bucket_list);
        bucket_menu.set_parent(&bucket_list_box);
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.pack_start(&icon_button(
            "list-add-symbolic",
            "New Bucket",
            "win.new-bucket",
        ));
        sidebar_header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&main_menu)
                .primary(true)
                .tooltip_text("Main Menu")
                .build(),
        );
        let profile_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label", "caption"])
            .build();
        let footer = gtk::Box::builder()
            .spacing(8)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        footer.append(
            &gtk::Image::builder()
                .icon_name("network-server-symbolic")
                .css_classes(["dim-label"])
                .build(),
        );
        footer.append(&profile_label);
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_header);
        sidebar_view.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .child(&bucket_list_box)
                .vexpand(true)
                .build(),
        ));
        sidebar_view.add_bottom_bar(&footer);
        let sidebar_page = adw::NavigationPage::builder()
            .title("Buckets")
            .child(&sidebar_view)
            .build();

        // ---- Content: objects
        let title = adw::WindowTitle::new("", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_start(&icon_button(
            "go-up-symbolic",
            "Parent Folder (Alt+Up)",
            "win.up",
        ));
        header.pack_start(
            &adw::SplitButton::builder()
                .label("Upload")
                .action_name("win.upload")
                .menu_model(&upload_menu)
                .tooltip_text("Upload Files (Ctrl+U)")
                .build(),
        );
        header.pack_start(&icon_button(
            "folder-new-symbolic",
            "New Folder (Ctrl+Shift+N)",
            "win.new-folder",
        ));
        let search_button = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search (Ctrl+F)")
            .build();
        header.pack_end(&search_button);
        let busy = gtk::Spinner::builder()
            .spinning(true)
            .visible(false)
            .tooltip_text("Working…")
            .build();
        header.pack_end(&busy);

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Filter this folder")
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(
                &adw::Clamp::builder()
                    .maximum_size(480)
                    .child(&search_entry)
                    .build(),
            )
            .build();
        search_bar.connect_entry(&search_entry);
        search_bar.set_key_capture_widget(Some(&window));
        search_button
            .bind_property("active", &search_bar, "search-mode-enabled")
            .bidirectional()
            .sync_create()
            .build();

        let objects = objects::ObjectsView::new(context_menu.upcast_ref());
        let details = details::DetailsPane::new();
        let details_split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .sidebar(&details.root)
            .content(&objects.root)
            .show_sidebar(false)
            .min_sidebar_width(300.0)
            .max_sidebar_width(380.0)
            .build();

        let selection_label = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["heading"])
            .build();
        let selection_bar = gtk::ActionBar::new();
        selection_bar.pack_start(&selection_label);
        let delete_button = labeled_button("user-trash-symbolic", "Delete", "win.delete");
        delete_button.add_css_class("destructive-action");
        selection_bar.pack_end(&delete_button);
        selection_bar.pack_end(&labeled_button(
            "document-edit-symbolic",
            "Rename",
            "win.rename",
        ));
        selection_bar.pack_end(&labeled_button(
            "emblem-shared-symbolic",
            "Share Link",
            "win.share",
        ));
        selection_bar.pack_end(&labeled_button(
            "folder-download-symbolic",
            "Download",
            "win.download",
        ));
        selection_bar.set_revealed(false);

        let transfers = transfers::TransfersBar::new();

        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&header);
        content_view.add_top_bar(&search_bar);
        content_view.set_content(Some(&details_split));
        content_view.add_bottom_bar(&selection_bar);
        content_view.add_bottom_bar(&transfers.root);
        let content_page = adw::NavigationPage::builder()
            .title("Objects")
            .child(&content_view)
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content_page)
            .min_sidebar_width(200.0)
            .max_sidebar_width(280.0)
            .build();

        // Adapt to smaller windows. Only one breakpoint applies at a time (the last matching one),
        // so the narrow one repeats the medium one's setters and is added after it.
        let medium = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 1000sp").expect("valid condition"),
        );
        medium.add_setter(&details_split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(medium);
        let narrow = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 640sp").expect("valid condition"),
        );
        narrow.add_setter(&details_split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(&selection_label, "visible", Some(&false.to_value()));
        window.add_breakpoint(narrow);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&connect.root, Some("connect"));
        stack.add_named(&split, Some("browser"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        window.set_content(Some(&toasts));

        let config = Config::load();
        let initial = config
            .last_profile
            .as_deref()
            .and_then(|n| config.find(n))
            .or(config.profiles.first())
            .cloned()
            .unwrap_or_default();
        connect.set_form(&initial);

        let win = Rc::new(Self {
            window,
            toasts,
            stack,
            connect,
            split,
            bucket_list,
            bucket_menu,
            profile_label,
            title,
            busy,
            search_bar,
            search_entry,
            objects,
            details,
            details_split,
            selection_bar,
            selection_label,
            transfers,
            state: RefCell::new(State {
                config,
                ..Default::default()
            }),
        });
        // Signal handlers only hold weak references; the window owns the strong one, so `Win` lives
        // exactly as long as its window.
        let keep_alive = RefCell::new(Some(win.clone()));
        win.window.connect_destroy(move |_| drop(keep_alive.take()));
        win.setup_actions(app);
        win.setup_help_overlay();
        win.connect_signals();
        win.refresh_profiles();
        win.update_selection();
        win
    }

    // ------------------------------------------------------------------ wiring

    fn add_action(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<Self>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let this = Rc::downgrade(self);
        action.connect_activate(move |_, _| {
            if let Some(this) = this.upgrade() {
                f(&this);
            }
        });
        self.window.add_action(&action);
    }

    fn add_string_action(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<Self>, String) + 'static) {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let this = Rc::downgrade(self);
        action.connect_activate(move |_, param| {
            if let (Some(this), Some(value)) =
                (this.upgrade(), param.and_then(|p| p.get::<String>()))
            {
                f(&this, value);
            }
        });
        self.window.add_action(&action);
    }

    fn set_action_enabled(&self, name: &str, enabled: bool) {
        if let Some(action) = self
            .window
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(enabled);
        }
    }

    fn setup_actions(self: &Rc<Self>, app: &adw::Application) {
        self.add_action("refresh", |w| {
            w.reload_buckets();
            w.reload();
        });
        self.add_action("up", |w| w.go_up());
        self.add_action("upload", |w| w.upload_files());
        self.add_action("upload-folder", |w| w.upload_folder());
        self.add_action("new-folder", |w| w.new_folder());
        self.add_action("new-bucket", |w| w.new_bucket());
        self.add_action("search", |w| {
            w.search_bar.set_search_mode(true);
            w.search_entry.grab_focus();
        });
        self.add_action("open", |w| {
            if let [e] = w.objects.selected().as_slice()
                && e.is_dir
            {
                w.open_entry(e);
            }
        });
        self.add_action("download", |w| w.download(w.objects.selected()));
        self.add_action("delete", |w| w.delete_selected());
        self.add_action("rename", |w| w.rename_selected());
        self.add_action("share", |w| w.share_selected());
        self.add_action("edit-properties", |w| w.edit_properties());
        self.add_action("copy-key", |w| {
            if let [e] = w.objects.selected().as_slice() {
                w.window.clipboard().set_text(&e.key);
                w.toast(&format!("Copied “{}”", e.key));
            }
        });
        self.add_action("disconnect", |w| w.disconnect());
        self.add_string_action("delete-bucket", |w, name| w.delete_bucket(name));
        self.add_string_action("copy-text", |w, text| {
            w.window.clipboard().set_text(&text);
            w.toast(&format!("Copied “{text}”"));
        });

        let flat = gio::SimpleAction::new_stateful("flat", None, &false.to_variant());
        let this = Rc::downgrade(self);
        flat.connect_activate(move |action, _| {
            let on = !action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            action.set_state(&on.to_variant());
            if let Some(w) = this.upgrade() {
                w.state.borrow_mut().recursive = on;
                w.reload();
            }
        });
        self.window.add_action(&flat);

        // App-level actions only need registering once.
        if app.lookup_action("about").is_none() {
            let about = gio::SimpleAction::new("about", None);
            let win = self.window.downgrade();
            about.connect_activate(move |_, _| {
                let dialog = adw::AboutDialog::builder()
                    .application_name(APP_NAME)
                    .application_icon(APP_ID)
                    .version(env!("CARGO_PKG_VERSION"))
                    .comments("Browse and manage S3-compatible object storage")
                    .developer_name("Arturo Martínez")
                    .license_type(gtk::License::MitX11)
                    .website("https://github.com/arturo-source/larder")
                    .issue_url("https://github.com/arturo-source/larder/issues")
                    .build();
                dialog.present(win.upgrade().as_ref());
            });
            app.add_action(&about);
            let quit = gio::SimpleAction::new("quit", None);
            let app_weak = app.downgrade();
            quit.connect_activate(move |_, _| {
                if let Some(app) = app_weak.upgrade() {
                    app.quit();
                }
            });
            app.add_action(&quit);
        }
    }

    fn setup_help_overlay(&self) {
        let group = |title: &str, items: &[(&str, &str)]| {
            let g = gtk::ShortcutsGroup::builder().title(title).build();
            for (accel, label) in items {
                g.add_shortcut(
                    &gtk::ShortcutsShortcut::builder()
                        .accelerator(*accel)
                        .title(*label)
                        .build(),
                );
            }
            g
        };
        let section = gtk::ShortcutsSection::builder()
            .section_name("main")
            .build();
        section.add_group(&group(
            "Browsing",
            &[
                ("<Alt>Up", "Parent folder"),
                ("F5", "Refresh"),
                ("<Control>f", "Search"),
                ("<Control>a", "Select all"),
            ],
        ));
        section.add_group(&group(
            "Objects",
            &[
                ("<Control>u", "Upload files"),
                ("<Control><Shift>n", "New folder"),
                ("<Control>s", "Download"),
                ("<Control>l", "Share link"),
                ("F2", "Rename"),
                ("Delete", "Delete"),
            ],
        ));
        section.add_group(&group(
            "General",
            &[("<Control>w", "Close window"), ("<Control>q", "Quit")],
        ));
        let overlay = gtk::ShortcutsWindow::builder().modal(true).build();
        overlay.add_section(&section);
        self.window.set_help_overlay(Some(&overlay));
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let with = move |f: fn(&Rc<Self>)| {
            let weak = weak.clone();
            move || {
                if let Some(w) = weak.upgrade() {
                    f(&w)
                }
            }
        };

        let f = with(|w| w.connect_form());
        self.connect.connect_button.connect_clicked(move |_| f());
        let f = with(|w| w.save_profile());
        self.connect.save_button.connect_clicked(move |_| f());
        let f = with(|w| w.reload());
        self.objects.retry_button.connect_clicked(move |_| f());
        let f = with(|w| w.download(w.objects.selected()));
        self.details.download_button.connect_clicked(move |_| f());
        let f = with(|w| w.share_selected());
        self.details.share_button.connect_clicked(move |_| f());
        let f = with(|w| w.edit_properties());
        self.details.edit_button.connect_clicked(move |_| f());
        let f = with(|w| w.update_selection());
        self.objects
            .selection
            .connect_selection_changed(move |_, _, _| f());
        // Replacing the listing removes selected items without a selection-changed signal.
        let f = with(|w| w.update_selection());
        self.objects
            .selection
            .connect_items_changed(move |_, _, _, _| f());

        let weak = Rc::downgrade(self);
        self.bucket_list.connect_row_selected(move |_, row| {
            let (Some(w), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            let name = {
                let st = w.state.borrow();
                // Also fired when we select the row ourselves; only react to real changes.
                st.buckets
                    .get(row.index() as usize)
                    .map(|b| b.name.clone())
                    .filter(|n| st.bucket.as_ref() != Some(n))
            };
            if let Some(name) = name {
                w.navigate(name, String::new());
            }
        });

        let gesture = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let weak = Rc::downgrade(self);
        gesture.connect_pressed(move |_, _, x, y| {
            let Some(w) = weak.upgrade() else { return };
            let Some(row) = w.bucket_list.row_at_y(y as i32) else {
                return;
            };
            let Some(name) = w
                .state
                .borrow()
                .buckets
                .get(row.index() as usize)
                .map(|b| b.name.clone())
            else {
                return;
            };
            let model = gio::Menu::new();
            for (label, action) in [
                ("Copy Name", "win.copy-text"),
                ("Delete Bucket…", "win.delete-bucket"),
            ] {
                let item = gio::MenuItem::new(Some(label), None);
                item.set_action_and_target_value(Some(action), Some(&name.to_variant()));
                model.append_item(&item);
            }
            w.bucket_menu.set_menu_model(Some(&model));
            w.bucket_menu
                .set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            w.bucket_menu.popup();
        });
        self.bucket_list.add_controller(gesture);

        let weak = Rc::downgrade(self);
        self.objects.column_view.connect_activate(move |_, pos| {
            let Some(w) = weak.upgrade() else { return };
            if let Some(e) = w.objects.entry_at(pos) {
                w.open_entry(&e);
            }
        });

        let weak = Rc::downgrade(self);
        self.objects.connect_drop(move |paths| {
            if let Some(w) = weak.upgrade() {
                w.upload_paths(paths);
            }
        });

        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |entry| {
            if let Some(w) = weak.upgrade() {
                w.objects.set_filter(&entry.text());
            }
        });
        let entry = self.search_entry.clone();
        self.search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if !bar.is_search_mode() {
                    entry.set_text("");
                }
            });
    }

    // ------------------------------------------------------------------ helpers

    fn toast(&self, message: &str) {
        self.toasts.add_toast(
            adw::Toast::builder()
                .title(message)
                .use_markup(false)
                .build(),
        );
    }

    fn error(&self, heading: &str, message: &str) {
        dialogs::error(&self.window, heading, message);
    }

    fn location(&self) -> Option<(Client, String, String)> {
        let st = self.state.borrow();
        Some((st.client.clone()?, st.bucket.clone()?, st.prefix.clone()))
    }

    fn set_pending(&self, delta: isize) {
        let pending = {
            let mut st = self.state.borrow_mut();
            st.pending = st.pending.saturating_add_signed(delta);
            st.pending
        };
        self.busy.set_visible(pending > 0);
    }

    /// Runs an S3 operation in the background: success shows a toast (unless the message is empty,
    /// meaning "nothing happened"), failure opens an error dialog titled `"<title> Failed"`.
    fn run_op<F>(self: &Rc<Self>, title: &'static str, refresh: Refresh, fut: F)
    where
        F: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        self.set_pending(1);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = bg(fut).await;
            this.set_pending(-1);
            match result {
                Ok(msg) if msg.is_empty() => {}
                Ok(msg) => this.toast(&msg),
                Err(e) => this.error(&format!("{title} Failed"), &format!("{e:#}")),
            }
            match refresh {
                Refresh::None => {}
                Refresh::Listing => this.reload(),
                Refresh::Buckets => this.reload_buckets(),
            }
        });
    }

    // ------------------------------------------------------------------ profiles & connection

    fn refresh_profiles(self: &Rc<Self>) {
        let profiles = self.state.borrow().config.profiles.clone();
        let weak = Rc::downgrade(self);
        // Clicking a saved profile connects right away; the form shows it in case it needs editing.
        let on_select = move |p: &Profile| {
            if let Some(w) = weak.upgrade() {
                w.connect.set_form(p);
                w.connect_with(p.clone());
            }
        };
        let weak = Rc::downgrade(self);
        let on_delete = move |name: &str| {
            let Some(w) = weak.upgrade() else { return };
            let name = name.to_string();
            let w2 = w.clone();
            dialogs::confirm(
                &w.window,
                "Delete Profile?",
                &format!("The saved connection “{name}” will be removed."),
                "Delete",
                true,
                move || {
                    let result = {
                        let mut st = w2.state.borrow_mut();
                        st.config.profiles.retain(|p| p.name != name);
                        st.config.save()
                    };
                    if let Err(e) = result {
                        w2.error("Couldn't Save Profiles", &format!("{e:#}"));
                    }
                    w2.refresh_profiles();
                },
            );
        };
        self.connect.set_profiles(&profiles, on_select, on_delete);
    }

    fn save_profile(self: &Rc<Self>) {
        let profile = self.connect.form();
        if profile.name.is_empty() {
            self.toast("Give the profile a name first");
            return;
        }
        let name = profile.name.clone();
        let result = {
            let mut st = self.state.borrow_mut();
            st.config.upsert(profile);
            st.config.last_profile = Some(name.clone());
            st.config.save()
        };
        match result {
            Ok(()) => self.toast(&format!("Saved profile “{name}”")),
            Err(e) => self.error("Couldn't Save Profile", &format!("{e:#}")),
        }
        self.refresh_profiles();
    }

    pub fn connect_profile(self: &Rc<Self>, name: &str) {
        let profile = self.state.borrow().config.find(name).cloned();
        match profile {
            Some(p) => {
                self.connect.set_form(&p);
                self.connect_with(p);
            }
            None => self.error(
                "Unknown Profile",
                &format!("There is no saved profile named “{name}”."),
            ),
        }
    }

    fn connect_form(self: &Rc<Self>) {
        let profile = self.connect.form();
        self.connect_with(profile);
    }

    fn connect_with(self: &Rc<Self>, profile: Profile) {
        self.connect.set_busy(true);
        let client = s3::make_client(&profile);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let c = client.clone();
            let result = bg(async move { s3::list_buckets(&c).await }).await;
            this.connect.set_busy(false);
            match result {
                Ok(buckets) => {
                    this.profile_label.set_label(&format!(
                        "{} · {}",
                        if profile.name.is_empty() {
                            "Unsaved profile"
                        } else {
                            &profile.name
                        },
                        connect::display_endpoint(&profile.endpoint)
                    ));
                    {
                        let mut st = this.state.borrow_mut();
                        st.client = Some(client);
                        st.profile = Some(profile);
                        st.bucket = None;
                        st.prefix.clear();
                    }
                    this.stack.set_visible_child_name("browser");
                    this.set_buckets(buckets);
                    let first = this.state.borrow().buckets.first().map(|b| b.name.clone());
                    match first {
                        Some(name) => this.navigate(name, String::new()),
                        None => this.show_no_bucket(),
                    }
                }
                Err(e) => this.error("Couldn't Connect", &format!("{e:#}")),
            }
        });
    }

    fn disconnect(self: &Rc<Self>) {
        {
            let mut st = self.state.borrow_mut();
            st.client = None;
            st.profile = None;
            st.buckets.clear();
            st.bucket = None;
            st.prefix.clear();
            st.listing_generation += 1;
            st.details_key = None;
        }
        self.bucket_list.remove_all();
        self.objects.set_entries(Vec::new());
        self.stack.set_visible_child_name("connect");
    }

    // ------------------------------------------------------------------ buckets

    fn set_buckets(&self, buckets: Vec<BucketInfo>) {
        self.bucket_list.remove_all();
        let current = self.state.borrow().bucket.clone();
        for b in &buckets {
            let row_box = gtk::Box::builder().spacing(12).build();
            row_box.append(&gtk::Image::from_icon_name("drive-harddisk-symbolic"));
            row_box.append(
                &gtk::Label::builder()
                    .label(&b.name)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            let row = gtk::ListBoxRow::builder().child(&row_box).build();
            if let Some(created) = &b.created {
                row.set_tooltip_text(Some(&format!("Created {created}")));
            }
            self.bucket_list.append(&row);
            if current.as_ref() == Some(&b.name) {
                self.bucket_list.select_row(Some(&row));
            }
        }
        self.state.borrow_mut().buckets = buckets;
    }

    fn reload_buckets(self: &Rc<Self>) {
        let Some(client) = self.state.borrow().client.clone() else {
            return;
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            match bg(async move { s3::list_buckets(&client).await }).await {
                Ok(buckets) => {
                    this.set_buckets(buckets);
                    let gone = {
                        let st = this.state.borrow();
                        st.bucket
                            .as_ref()
                            .is_some_and(|b| !st.buckets.iter().any(|x| &x.name == b))
                    };
                    if gone {
                        this.state.borrow_mut().bucket = None;
                        this.show_no_bucket();
                    }
                }
                Err(e) => this.error("Couldn't List Buckets", &format!("{e:#}")),
            }
        });
    }

    fn show_no_bucket(&self) {
        let has_buckets = !self.state.borrow().buckets.is_empty();
        self.title.set_title("");
        self.title.set_subtitle("");
        self.objects.set_drop_destination(None);
        if has_buckets {
            self.objects.show_placeholder(
                "drive-harddisk-symbolic",
                "No Bucket Selected",
                "Pick a bucket from the sidebar",
            );
        } else {
            self.objects.show_placeholder(
                "drive-harddisk-symbolic",
                "No Buckets Yet",
                "Create one with the + button in the sidebar",
            );
        }
        self.update_actions();
    }

    fn new_bucket(self: &Rc<Self>) {
        if self.state.borrow().client.is_none() {
            return;
        }
        let this = self.clone();
        dialogs::ask_text(
            &self.window,
            "New Bucket",
            "Bucket names must be unique on the server.",
            "Bucket Name",
            "",
            None,
            "Create",
            |name| {
                if valid_bucket_name(name) {
                    Ok(())
                } else {
                    Err("Use 3–63 lowercase letters, digits, hyphens or dots, starting and ending with a letter or digit".into())
                }
            },
            move |name| {
                let Some(client) = this.state.borrow().client.clone() else {
                    return;
                };
                let region = this
                    .state
                    .borrow()
                    .profile
                    .as_ref()
                    .map(|p| p.region.clone())
                    .unwrap_or_default();
                let this = this.clone();
                this.set_pending(1);
                glib::spawn_future_local(async move {
                    let n = name.clone();
                    let result = bg(async move {
                        s3::create_bucket(&client, &n, &region).await?;
                        s3::list_buckets(&client).await
                    })
                    .await;
                    this.set_pending(-1);
                    match result {
                        Ok(buckets) => {
                            this.set_buckets(buckets);
                            this.toast(&format!("Created bucket “{name}”"));
                            this.navigate(name, String::new());
                        }
                        Err(e) => this.error("Create Bucket Failed", &format!("{e:#}")),
                    }
                });
            },
        );
    }

    fn delete_bucket(self: &Rc<Self>, name: String) {
        let this = self.clone();
        dialogs::delete_bucket(&self.window, &name.clone(), move |force| {
            let Some(client) = this.state.borrow().client.clone() else {
                return;
            };
            let name = name.clone();
            this.run_op("Delete Bucket", Refresh::Buckets, async move {
                let n = s3::delete_bucket(&client, &name, force).await?;
                Ok(if n > 0 {
                    format!("Deleted bucket “{name}” and {n} objects")
                } else {
                    format!("Deleted bucket “{name}”")
                })
            });
        });
    }

    // ------------------------------------------------------------------ browsing

    fn navigate(self: &Rc<Self>, bucket: String, prefix: String) {
        let changed = {
            let mut st = self.state.borrow_mut();
            let changed = st.bucket.as_ref() != Some(&bucket) || st.prefix != prefix;
            st.bucket = Some(bucket.clone());
            st.prefix = prefix;
            changed
        };
        if changed {
            self.search_bar.set_search_mode(false);
        }
        let index = self
            .state
            .borrow()
            .buckets
            .iter()
            .position(|b| b.name == bucket);
        if let Some(row) = index.and_then(|i| self.bucket_list.row_at_index(i as i32)) {
            self.bucket_list.select_row(Some(&row));
        }
        self.split.set_show_content(true);
        self.reload();
    }

    fn reload(self: &Rc<Self>) {
        let Some((client, bucket, prefix)) = self.location() else {
            return;
        };
        let (generation, recursive) = {
            let mut st = self.state.borrow_mut();
            st.listing_generation += 1;
            (st.listing_generation, st.recursive)
        };
        self.objects.show_loading();
        self.objects
            .set_drop_destination(Some(&format!("{bucket}/{prefix}")));
        self.update_title();
        self.update_actions();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result =
                bg(async move { s3::list(&client, &bucket, &prefix, recursive).await }).await;
            let current = this.state.borrow().listing_generation;
            if current != generation {
                return; // a newer listing was started meanwhile
            }
            match result {
                Ok(entries) => {
                    this.objects.set_entries(entries);
                    this.update_title();
                    // After picking a bucket in the sidebar, keyboard focus moves to its contents.
                    if gtk::prelude::RootExt::focus(&this.window)
                        .is_some_and(|f| f.is_ancestor(&this.bucket_list))
                    {
                        this.objects.column_view.grab_focus();
                    }
                }
                Err(e) => this.objects.show_error(&format!("{e:#}")),
            }
        });
    }

    fn update_title(&self) {
        let (bucket, prefix) = {
            let st = self.state.borrow();
            (st.bucket.clone().unwrap_or_default(), st.prefix.clone())
        };
        self.title.set_title(if prefix.is_empty() {
            &bucket
        } else {
            base_name(&prefix)
        });
        let entries = self.objects.entries();
        let path = format!("{bucket}/{prefix}");
        if entries.is_empty() {
            self.title.set_subtitle(&path);
        } else {
            let total: i64 = entries.iter().map(|e| e.size).sum();
            let n = entries.len();
            self.title.set_subtitle(&format!(
                "{path} · {n} item{} · {}",
                if n == 1 { "" } else { "s" },
                human_size(total)
            ));
        }
    }

    fn go_up(self: &Rc<Self>) {
        let (bucket, prefix) = {
            let st = self.state.borrow();
            (st.bucket.clone(), st.prefix.clone())
        };
        let Some(bucket) = bucket else { return };
        if prefix.is_empty() {
            return;
        }
        let trimmed = prefix.trim_end_matches('/');
        let parent = trimmed
            .rfind('/')
            .map(|i| trimmed[..=i].to_string())
            .unwrap_or_default();
        self.navigate(bucket, parent);
    }

    fn open_entry(self: &Rc<Self>, e: &Entry) {
        if e.is_dir {
            let bucket = self.state.borrow().bucket.clone();
            if let Some(bucket) = bucket {
                self.navigate(bucket, e.key.clone());
            }
        } else {
            self.download(vec![e.clone()]);
        }
    }

    fn update_actions(&self) {
        let (connected, in_bucket, can_go_up) = {
            let st = self.state.borrow();
            (
                st.client.is_some(),
                st.bucket.is_some(),
                !st.prefix.is_empty(),
            )
        };
        for name in ["upload", "upload-folder", "new-folder", "search", "refresh"] {
            self.set_action_enabled(name, in_bucket);
        }
        self.set_action_enabled("up", can_go_up);
        self.set_action_enabled("new-bucket", connected);
        self.set_action_enabled("disconnect", connected);
    }

    fn update_selection(self: &Rc<Self>) {
        let selected = self.objects.selected();
        let single = if let [e] = selected.as_slice() {
            Some(e.clone())
        } else {
            None
        };
        let n = selected.len();
        self.selection_bar.set_revealed(n > 0);
        match &single {
            Some(e) => self.selection_label.set_label(base_name(&e.key)),
            None => self
                .selection_label
                .set_label(&format!("{n} items selected")),
        }
        self.set_action_enabled("download", n > 0);
        self.set_action_enabled("delete", n > 0);
        self.set_action_enabled("rename", single.is_some());
        self.set_action_enabled("copy-key", single.is_some());
        self.set_action_enabled("share", single.as_ref().is_some_and(|e| !e.is_dir));
        self.set_action_enabled(
            "edit-properties",
            single.as_ref().is_some_and(|e| !e.is_dir),
        );
        self.set_action_enabled("open", single.as_ref().is_some_and(|e| e.is_dir));
        self.update_actions();

        match single.filter(|e| !e.is_dir) {
            Some(e) => self.load_details(e),
            None => {
                self.state.borrow_mut().details_key = None;
                self.details_split.set_show_sidebar(false);
            }
        }
    }

    fn load_details(self: &Rc<Self>, e: Entry) {
        if self.state.borrow().details_key.as_ref() == Some(&e.key) {
            return;
        }
        self.state.borrow_mut().details_key = Some(e.key.clone());
        self.details.show_loading(&e);
        self.details_split.set_show_sidebar(true);
        let Some((client, bucket, _)) = self.location() else {
            return;
        };
        let this = self.clone();
        let key = e.key.clone();
        glib::spawn_future_local(async move {
            let k = key.clone();
            let result = bg(async move {
                let details = s3::head(&client, &bucket, &k).await?;
                let preview = s3::preview(&client, &bucket, &details)
                    .await
                    .map_err(|e| format!("{e:#}"));
                anyhow::Ok((details, preview))
            })
            .await;
            if this.state.borrow().details_key.as_ref() != Some(&key) {
                return;
            }
            match result {
                Ok((details, preview)) => this.details.show(&details, &preview),
                Err(e) => this.details.show_error(&format!("{e:#}")),
            }
        });
    }

    // ------------------------------------------------------------------ object operations

    fn upload_files(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Upload Files")
                .modal(true)
                .build();
            let Ok(files) = dialog.open_multiple_future(Some(&this.window)).await else {
                return;
            };
            let paths: Vec<PathBuf> = (0..files.n_items())
                .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                .filter_map(|f| f.path())
                .collect();
            this.upload_paths(paths);
        });
    }

    fn upload_folder(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Upload Folder")
                .modal(true)
                .build();
            let Ok(folder) = dialog.select_folder_future(Some(&this.window)).await else {
                return;
            };
            if let Some(path) = folder.path() {
                this.upload_paths(vec![path]);
            }
        });
    }

    fn upload_paths(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let Some((client, bucket, prefix)) = self.location() else {
            self.toast("Select a bucket first");
            return;
        };
        let progress = Progress::new(format!("Uploading to {bucket}/{prefix}"));
        self.transfers.add(progress.clone());
        self.run_op("Upload", Refresh::Listing, async move {
            let _finish = FinishOnDrop(progress.clone());
            let plan_prefix = prefix.clone();
            let items = tokio::task::spawn_blocking(move || s3::plan_upload(&paths, &plan_prefix))
                .await??;
            match s3::upload_all(&client, &bucket, &items, &progress).await {
                Ok((files, bytes)) => Ok(format!(
                    "Uploaded {}, {}",
                    count(files, "file"),
                    human_size(bytes as i64)
                )),
                Err(_) if progress.is_cancelled() => Ok(format!(
                    "Upload cancelled after {} of {} files",
                    progress.done_files.load(Ordering::Relaxed),
                    items.len()
                )),
                Err(e) => Err(e),
            }
        });
    }

    fn download(self: &Rc<Self>, targets: Vec<Entry>) {
        if targets.is_empty() {
            return;
        }
        let Some((client, bucket, _)) = self.location() else {
            return;
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            // A single file gets a "save as" dialog; anything else goes into a chosen folder.
            if let [e] = targets.as_slice()
                && !e.is_dir
            {
                let dialog = gtk::FileDialog::builder()
                    .title("Save Object")
                    .initial_name(base_name(&e.key))
                    .modal(true)
                    .build();
                let Ok(file) = dialog.save_future(Some(&this.window)).await else {
                    return;
                };
                let Some(dest) = file.path() else { return };
                let key = e.key.clone();
                this.run_op("Download", Refresh::None, async move {
                    let n = s3::download(&client, &bucket, &key, &dest).await?;
                    Ok(format!(
                        "Downloaded “{}”, {}",
                        base_name(&key),
                        human_size(n as i64)
                    ))
                });
                return;
            }
            let dialog = gtk::FileDialog::builder()
                .title("Download to Folder")
                .modal(true)
                .build();
            let Ok(folder) = dialog.select_folder_future(Some(&this.window)).await else {
                return;
            };
            let Some(dir) = folder.path() else { return };
            this.run_op("Download", Refresh::None, async move {
                let (mut files, mut bytes) = (0, 0);
                for e in &targets {
                    let dest = dir.join(base_name(&e.key));
                    if e.is_dir {
                        let (n, b) = s3::download_prefix(&client, &bucket, &e.key, &dest).await?;
                        files += n;
                        bytes += b;
                    } else {
                        bytes += s3::download(&client, &bucket, &e.key, &dest).await?;
                        files += 1;
                    }
                }
                Ok(format!(
                    "Downloaded {}, {} to {}",
                    count(files, "file"),
                    human_size(bytes as i64),
                    dir.display()
                ))
            });
        });
    }

    fn delete_selected(self: &Rc<Self>) {
        let targets = self.objects.selected();
        if targets.is_empty() {
            return;
        }
        let heading = match targets.as_slice() {
            [e] => format!("Delete “{}”?", base_name(&e.key)),
            _ => format!("Delete {} Items?", targets.len()),
        };
        let mut body = "This can't be undone.".to_string();
        if targets.iter().any(|e| e.is_dir) {
            body.push_str(" Folders are deleted together with everything inside them.");
        }
        let this = self.clone();
        dialogs::confirm(&self.window, &heading, &body, "Delete", true, move || {
            let Some((client, bucket, _)) = this.location() else {
                return;
            };
            let targets: Vec<(String, bool)> =
                targets.iter().map(|e| (e.key.clone(), e.is_dir)).collect();
            this.run_op("Delete", Refresh::Listing, async move {
                let n = s3::delete_targets(&client, &bucket, &targets).await?;
                Ok(format!("Deleted {}", count(n, "object")))
            });
        });
    }

    fn rename_selected(self: &Rc<Self>) {
        let selected = self.objects.selected();
        let [e] = selected.as_slice() else { return };
        let from = e.key.clone();
        let is_dir = e.is_dir;
        let shown = from.trim_end_matches('/').to_string();
        // Pre-select the name (without extension) so typing replaces just that.
        let name_start = shown.rfind('/').map(|i| i + 1).unwrap_or(0);
        let name_end = if is_dir {
            shown.len()
        } else {
            shown
                .rfind('.')
                .filter(|&i| i > name_start)
                .unwrap_or(shown.len())
        };
        let this = self.clone();
        let original = shown.clone();
        dialogs::ask_text(
            &self.window,
            if is_dir { "Rename Folder" } else { "Rename" },
            "Include slashes to move it into another folder.",
            "New Key",
            &shown,
            Some((
                shown[..name_start].chars().count() as i32,
                shown[..name_end].chars().count() as i32,
            )),
            "Rename",
            move |to| {
                let to = to.trim();
                if to.is_empty() || to.trim_matches('/').is_empty() {
                    Err("The name can't be empty".into())
                } else if to == original {
                    Err("That's the current name".into())
                } else {
                    Ok(())
                }
            },
            move |to| {
                let Some((client, bucket, _)) = this.location() else {
                    return;
                };
                let mut to = to.trim().trim_start_matches('/').to_string();
                if is_dir && !to.ends_with('/') {
                    to.push('/');
                }
                let from = from.clone();
                this.run_op("Rename", Refresh::Listing, async move {
                    s3::rename(&client, &bucket, &from, &to).await?;
                    Ok(format!("Renamed to “{}”", to.trim_end_matches('/')))
                });
            },
        );
    }

    fn edit_properties(self: &Rc<Self>) {
        let selected = self.objects.selected();
        let [entry] = selected.as_slice() else { return };
        let Some(details) = self.details.current().filter(|d| d.key == entry.key) else {
            self.toast("The properties are still loading");
            return;
        };
        let this = self.clone();
        let entry = entry.clone();
        properties::edit_properties(&self.window, &details.clone(), move |edit| {
            let Some((client, bucket, _)) = this.location() else {
                return;
            };
            let (this, entry, details) = (this.clone(), entry.clone(), details.clone());
            this.set_pending(1);
            glib::spawn_future_local(async move {
                let result =
                    bg(async move { s3::update_object(&client, &bucket, &details, &edit).await })
                        .await;
                this.set_pending(-1);
                match result {
                    Ok(()) => {
                        this.toast(&format!("Updated “{}”", base_name(&entry.key)));
                        this.state.borrow_mut().details_key = None;
                        this.load_details(entry);
                        this.reload();
                    }
                    Err(e) => this.error("Couldn't Update Properties", &format!("{e:#}")),
                }
            });
        });
    }

    fn share_selected(self: &Rc<Self>) {
        let selected = self.objects.selected();
        let [e] = selected.as_slice() else { return };
        if e.is_dir {
            return;
        }
        let this = self.clone();
        let key = e.key.clone();
        let name = base_name(&key).to_string();
        dialogs::share_link(&self.window, &name, move |expires, label| {
            let Some((client, bucket, _)) = this.location() else {
                return;
            };
            let key = key.clone();
            let this2 = this.clone();
            glib::spawn_future_local(async move {
                match bg(async move { s3::presign(&client, &bucket, &key, expires).await }).await {
                    Ok(url) => {
                        this2.window.clipboard().set_text(&url);
                        this2.toast(&format!("Link copied to clipboard, valid for {label}"));
                    }
                    Err(e) => this2.error("Couldn't Create Link", &format!("{e:#}")),
                }
            });
        });
    }

    fn new_folder(self: &Rc<Self>) {
        let this = self.clone();
        dialogs::ask_text(
            &self.window,
            "New Folder",
            "",
            "Folder Name",
            "",
            None,
            "Create",
            |name| {
                if name.trim().trim_matches('/').is_empty() {
                    Err("The name can't be empty".into())
                } else {
                    Ok(())
                }
            },
            move |name| {
                let Some((client, bucket, prefix)) = this.location() else {
                    return;
                };
                let key = format!("{prefix}{}/", name.trim().trim_matches('/'));
                this.run_op("Create Folder", Refresh::Listing, async move {
                    s3::create_folder(&client, &bucket, &key).await?;
                    Ok(format!("Created folder “{}”", base_name(&key)))
                });
            },
        );
    }
}

/// Marks a transfer finished however its task ends, so the transfers bar never shows a stale entry.
struct FinishOnDrop(Arc<Progress>);

impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        self.0.finished.store(true, Ordering::Relaxed);
    }
}

/// "1 file", "3 files".
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn valid_bucket_name(n: &str) -> bool {
    (3..=63).contains(&n.len())
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && n.starts_with(|c: char| c.is_ascii_alphanumeric())
        && n.ends_with(|c: char| c.is_ascii_alphanumeric())
}
