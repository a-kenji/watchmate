use std::sync::Arc;
use futures::StreamExt;
use gtk::prelude::{BoxExt, OrientableExt, WidgetExt};
use relm4::{gtk, ComponentParts, ComponentSender, Component, WidgetPlus, JoinHandle};
use mpris2_zbus::media_player::MediaPlayer;
use zbus::names::OwnedBusName;

use crate::{bt, media_player as mp};

#[derive(Debug)]
pub enum Input {
    Device(Option<Arc<bt::InfiniTime>>),
    PlayerControlSessionStart,
    PlayerControlSessionEnded,
    PlayerUpdateSessionStart,
    PlayerUpdateSessionEnded,
    PlayerAdded(MediaPlayer),
    PlayerRemoved(OwnedBusName),
}

#[derive(Debug)]
pub enum Output {
}

#[derive(Debug)]
pub enum CommandOutput {
    None,
    DBusConnection(zbus::Connection),
}

#[derive(Default)]
pub struct Model {
    player_handles: Vec<Arc<MediaPlayer>>,
    player_names: gtk::StringList,
    infinitime: Option<Arc<bt::InfiniTime>>,
    control_task: Option<JoinHandle<()>>,
    update_task: Option<JoinHandle<()>>,
    dbus_session: Option<Arc<zbus::Connection>>,
    dropdown: gtk::DropDown,
}


impl Model {
    fn stop_control_task(&mut self) {
        if self.control_task.take().map(|h| h.abort()).is_some() {
            log::info!("Media Player Control session stopped");
        }
    }

    fn stop_update_task(&mut self) {
        if self.update_task.take().map(|h| h.abort()).is_some() {
            log::info!("Media Player List Update session stopped");
        }
    }
}

#[relm4::component(pub)]
impl Component for Model {
    type CommandOutput = CommandOutput;
    type Init = ();
    type Input = Input;
    type Output = Output;
    type Widgets = Widgets;

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_margin_all: 12,
            set_spacing: 10,

            if model.player_handles.is_empty() {
                gtk::Label {
                    set_hexpand: true,
                    set_label: "No media players detected",
                }
            } else {
                #[local]
                dropdown -> gtk::DropDown {
                    set_hexpand: true,
                    #[watch]
                    set_model: Some(&model.player_names),
                    connect_selected_notify[sender] => move |_| {
                        sender.input(Input::PlayerControlSessionStart);
                    }
                }
            }
        }
    }

    fn init(_: Self::Init, root: &Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let dropdown = gtk::DropDown::default();
        let model = Self { dropdown: dropdown.clone(), ..Default::default() };
        let widgets = view_output!();
        sender.oneshot_command(async move {
            match zbus::Connection::session().await {
                Ok(connection) => {
                    CommandOutput::DBusConnection(connection)
                }
                Err(error) => {
                    log::error!("Failed to establish D-Bus session connection: {error}");
                    CommandOutput::None
                }
            }
        });
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            Input::Device(infinitime) => {
                self.infinitime = infinitime;
                if self.infinitime.is_some() {
                    sender.input(Input::PlayerControlSessionStart);
                }
            }
            Input::PlayerControlSessionStart => {
                if let Some(infinitime) = self.infinitime.clone() {
                    let index = self.dropdown.selected() as usize;
                    if index < self.player_handles.len() {
                        // Stop current media player control sesssion
                        self.stop_control_task();
                        // Start new media player control sesssion
                        let player = self.player_handles[index].clone();
                        let task_handle = relm4::spawn(async move {
                            match mp::run_control_session(&player, &infinitime).await {
                                Ok(()) => log::warn!("Media player control session ended unexpectedly"),
                                Err(error) => log::error!("Media player control session error: {error}"),
                            }
                            sender.input(Input::PlayerControlSessionEnded);
                        });
                        self.control_task = Some(task_handle);
                    }
                }
            }
            Input::PlayerControlSessionEnded => {
                self.player_handles.clear();
                self.player_names = gtk::StringList::new(&[]);
                self.control_task = None;
            }
            Input::PlayerUpdateSessionStart => {
                if let Some(dbus_session) = self.dbus_session.clone() {
                    self.stop_update_task();
                    let task_handle = relm4::spawn(async move {
                        match mp::get_players_update_stream(&dbus_session).await {
                            Ok(stream) => stream.for_each(|event| {
                                let dbus_session_ = dbus_session.clone();
                                let sender_ = sender.clone();
                                async move {
                                    match event {
                                        mp::PlayersListEvent::PlayerAdded(bus) => {
                                            if let Ok(player) = MediaPlayer::new(&dbus_session_, bus).await {
                                                let _ = player.identity().await; // Cache player name
                                                sender_.input(Input::PlayerAdded(player));
                                            }
                                        }
                                        mp::PlayersListEvent::PlayerRemoved(bus) => {
                                            sender_.input(Input::PlayerRemoved(bus));
                                        }
                                    }
                                }
                            }).await,
                            Err(error) => log::error!("Failed to start player list update session: {error}"),
                        }
                        sender.input(Input::PlayerUpdateSessionEnded);
                    });
                    self.update_task = Some(task_handle);
                }
            }
            Input::PlayerUpdateSessionEnded => {
                log::info!("Restarting player list update session");
                sender.input(Input::PlayerUpdateSessionStart);
            }
            Input::PlayerAdded(player) => {
                if let Ok(Some(name)) = player.cached_identity() {
                    self.player_names.append(&name);
                    self.player_handles.push(Arc::new(player));
                    log::info!("Player started: {name}");
                } else {
                    log::error!("Failed to obtain cached player identity");
                }
            }
            Input::PlayerRemoved(bus) => {
                if let Some(index) = self.player_handles.iter().position(|p| p.destination() == &bus) {
                    let name = self.player_names.string(index as u32).unwrap();
                    self.player_names.remove(index as u32);
                    self.player_handles.remove(index);
                    log::info!("Player stopped: {name}");
                    if self.player_handles.is_empty() {
                        self.stop_control_task();
                    }
                }
            }
        }
    }

    fn update_cmd(&mut self, msg: Self::CommandOutput, sender: ComponentSender<Self>) {
        match msg {
            CommandOutput::None => {}
            CommandOutput::DBusConnection(connection) => {
                self.dbus_session = Some(Arc::new(connection));
                sender.input(Input::PlayerUpdateSessionStart);
            }
        }
    }
}

