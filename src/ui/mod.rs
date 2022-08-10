use std::{sync::Arc, path::PathBuf};
use gtk::prelude::{BoxExt, GtkWindowExt};
use relm4::{
    adw, gtk, Component, ComponentController, ComponentParts, ComponentSender, Controller, RelmApp
};
use relm4_components::open_dialog::*;

use crate::bt;

mod dashboard;
mod devices;
mod fwupd;

#[derive(Debug)]
enum Input {
    SetView(View),
    DeviceConnected(Arc<bluer::Device>),
    DeviceDisconnected(Arc<bluer::Device>),
    FirmwareUpdateFileChooser,
    FirmwareUpdateFromFile(PathBuf),
    FirmwareUpdateFromUrl(String),
    Notification(String),
    Ignore,
}

#[derive(Debug)]
enum CommandOutput {
    DeviceReady(Arc<bt::InfiniTime>),
}

struct Model {
    // UI state
    active_view: View,
    is_connected: bool,
    // Components
    dashboard: Controller<dashboard::Model>,
    devices: Controller<devices::Model>,
    fwupd: Controller<fwupd::Model>,
    fwupd_file_chooser: Controller<OpenDialog>,
    // Other
    infinitime: Option<Arc<bt::InfiniTime>>,
    toast_overlay: adw::ToastOverlay,
}

impl Model {
    fn notify(&self, message: &str) {
        self.toast_overlay.add_toast(&adw::Toast::new(message));
    }
}

#[relm4::component]
impl Component for Model {
    type CommandOutput = CommandOutput;
    type InitParams = Arc<bluer::Adapter>;
    type Input = Input;
    type Output = ();
    type Widgets = Widgets;

    view! {
        adw::ApplicationWindow {
            set_default_width: 320,
            set_default_height: 568,

            #[local]
            toast_overlay -> adw::ToastOverlay {
                // TODO: Use Relm 0.5 conditional widgets here (automatic stack)
                // I can't make it work here for some reason for now.
                #[wrap(Some)]
                set_child = &gtk::Stack {
                    add_named[Some("dashboard_view")] = &gtk::Box {
                        // set_visible: watch!(components.dashboard.model.device.is_some()),
                        append: model.dashboard.widget(),
                    },
                    add_named[Some("devices_view")] = &gtk::Box {
                        append: model.devices.widget(),
                    },
                    add_named[Some("fwupd_view")] = &gtk::Box {
                        append: model.fwupd.widget(),
                    },
                    #[watch]
                    set_visible_child_name: match model.active_view {
                        View::Dashboard => "dashboard_view",
                        View::Devices => "devices_view",
                        View::FirmwareUpdate => "fwupd_view",
                    },
                },
            },
        }
    }

    fn init(adapter: Self::InitParams, root: &Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        // Components
        let dashboard = dashboard::Model::builder()
            .launch(())
            .forward(&sender.input, |message| match message {
                dashboard::Output::FirmwareUpdateFromFile => Input::FirmwareUpdateFileChooser,
                dashboard::Output::FirmwareUpdateFromUrl(url) => Input::FirmwareUpdateFromUrl(url),
                dashboard::Output::Notification(text) => Input::Notification(text),
                dashboard::Output::SetView(view) => Input::SetView(view),
            });

        let devices = devices::Model::builder()
            .launch(adapter)
            .forward(&sender.input, |message| match message {
                devices::Output::DeviceConnected(device) => Input::DeviceConnected(device),
                devices::Output::DeviceDisconnected(device) => Input::DeviceDisconnected(device),
                devices::Output::Notification(text) => Input::Notification(text),
                devices::Output::SetView(view) => Input::SetView(view),
            });

        let fwupd = fwupd::Model::builder()
            .launch(())
            .forward(&sender.input, |message| match message {
                fwupd::Output::SetView(view) => Input::SetView(view),
            });

        let file_filter = gtk::FileFilter::new();
        file_filter.add_pattern("*.zip");
        let fwupd_file_chooser = OpenDialog::builder()
            .transient_for_native(root)
            .launch(OpenDialogSettings {
                create_folders: false,
                filters: vec![file_filter],
                ..Default::default()
            })
            .forward(&sender.input, |message| match message {
                OpenDialogResponse::Accept(path) => Input::FirmwareUpdateFromFile(path),
                OpenDialogResponse::Cancel => Input::Ignore,
            });

        let toast_overlay = adw::ToastOverlay::new();

        let model = Model {
            // UI state
            active_view: View::Devices,
            is_connected: false,
            // Components
            dashboard,
            devices,
            fwupd,
            fwupd_file_chooser,
            // Other
            infinitime: None,
            toast_overlay: toast_overlay.clone(),
        };

        let widgets = view_output!();

        ComponentParts { model, widgets }
    }


    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            Input::SetView(view) => {
                self.active_view = view;
            }
            Input::DeviceConnected(device) => {
                self.is_connected = true;
                sender.clone().command(move |out, shutdown| {
                    let task = async move {
                        match bt::InfiniTime::new(device).await {
                            Ok(infinitime) => {
                                out.send(CommandOutput::DeviceReady(Arc::new(infinitime)));
                            }
                            Err(error) => {
                                eprintln!("Failed to connect to InfiniTime: {}", error);
                                sender.input(Input::Notification(format!("Failed to connect to the watch")));
                            }
                        }
                    };
                    shutdown.register(task).drop_on_shutdown()
                })
            }
            Input::DeviceDisconnected(device) => {
                if self.infinitime.as_ref().map_or(false, |i| i.device().address() == device.address()) {
                // Use this once is_some_and is stabilized:
                // if self.infinitime.is_some_and(|i| i.device().address() == device.address()) {
                    self.infinitime = None;
                }
                self.dashboard.emit(dashboard::Input::Disconnected);
                self.fwupd.emit(fwupd::Input::Disconnected);
            }
            Input::FirmwareUpdateFileChooser => {
                self.fwupd_file_chooser.emit(OpenDialogMsg::Open);
            }
            Input::FirmwareUpdateFromFile(filepath) => {
                self.fwupd.emit(fwupd::Input::FirmwareUpdateFromFile(filepath));
                sender.input(Input::SetView(View::FirmwareUpdate));
            }
            Input::FirmwareUpdateFromUrl(url) => {
                self.fwupd.emit(fwupd::Input::FirmwareUpdateFromUrl(url));
                sender.input(Input::SetView(View::FirmwareUpdate));
            }
            Input::Notification(message) => {
                self.notify(&message);
            }
            Input::Ignore => {}
        }
    }

    fn update_cmd(&mut self, msg: Self::CommandOutput, _sender: ComponentSender<Self>) {
        match msg {
            CommandOutput::DeviceReady(infinitime) => {
                self.infinitime = Some(infinitime.clone());
                self.active_view = View::Dashboard;
                self.dashboard.emit(dashboard::Input::Connected(infinitime.clone()));
                self.fwupd.emit(fwupd::Input::Connected(infinitime));
            }
        }
    }
}



#[derive(Debug, PartialEq)]
pub enum View {
    Dashboard,
    Devices,
    FirmwareUpdate,
}


pub fn run(adapter: Arc<bluer::Adapter>) {
    // Init GTK before libadwaita (ToastOverlay)
    gtk::init().unwrap();

    // Run app
    let app = RelmApp::new("io.gitlab.azymohliad.WatchMate");
    app.run::<Model>(adapter);
}
