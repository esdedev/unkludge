//! What follows from the worker's events, whichever interface is showing them: the state, the commands
//! that come of it, the report, and a scripted demo moving along. What's left for the interface is in
//! [`Drained`], for it to act on in its own way.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::demo::{Script, Step};
use crate::session::Session;
use crate::settings::Settings;
use crate::state::State;
use crate::worker::{Cmd, Event, ReportData};

/// How long passing news stays at the foot of the window.
pub const MESSAGE_TIME: Duration = Duration::from_millis(6000);
/// A chunk can go every few milliseconds, and the bar needn't show every one.
pub const PROGRESS_PAINT_GAP: Duration = Duration::from_millis(250);

/// What it takes, of what the interface holds.
pub struct Parts<'a> {
    pub state: &'a mut State,
    pub settings: &'a mut Settings,
    pub session: &'a Session,
    pub script: &'a mut Option<Script>,
    /// Where a report goes.
    pub data_dir: Option<PathBuf>,
    /// Passes a command on to the worker.
    pub send: &'a mut dyn FnMut(Cmd),
}

/// What's left for the interface to do once the events have been read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// An upload was going, and now none is.
    pub upload_ended: bool,
    /// The picture to show is another, or gone: its preview starts over.
    pub image_changed: bool,
    /// Nothing but an upload moving along came, so a redraw can wait a little.
    pub only_progress: bool,
    /// The message at the foot of the window is a new one, and whether it goes away by itself.
    pub message_fades: Option<bool>,
    /// The settings changed.
    pub save_settings: bool,
    /// Folders and pages to open, which can take a while.
    pub to_open: Vec<PathBuf>,
}

impl Parts<'_> {
    /// Reads every event that's waiting.
    pub fn drain(&mut self, events: &Receiver<Event>) -> Drained {
        let mut drained = Drained {
            only_progress: true,
            ..Drained::default()
        };
        let was_uploading = !self.state.upload.is_idle();
        let message_before = self.state.message.clone();
        while let Ok(event) = events.try_recv() {
            drained.only_progress &= matches!(event, Event::Progress { .. });
            drained.image_changed |= matches!(
                event,
                Event::ImageLoaded(_) | Event::Disconnected | Event::ImageFailed(_)
            );
            if cfg!(debug_assertions) {
                print_event(&event);
            }
            self.take_event(event, &mut drained);
        }
        drained.upload_ended = was_uploading && self.state.upload.is_idle();
        if self.state.message != message_before {
            drained.message_fades = self.state.message.as_ref().map(|message| message.fades());
        }
        drained
    }

    fn take_event(&mut self, event: Event, drained: &mut Drained) {
        let steps = match self.script.as_mut() {
            Some(script) => script.on_event(&event),
            None => Vec::new(),
        };
        let reaction = self.state.apply(event, self.settings);
        for cmd in reaction.cmds {
            (self.send)(cmd);
        }
        drained.save_settings |= reaction.save_settings;
        if let Some(data) = reaction.report {
            self.report_ready(&data, drained);
        }
        for step in steps {
            match step {
                Step::ChooseFile(path) => {
                    if let Some(cmd) = self.state.choose_file(path) {
                        (self.send)(cmd);
                    }
                }
                Step::Upload => {
                    if let Some(cmd) = self.state.start_upload() {
                        (self.send)(cmd);
                    }
                }
                Step::Cancel => {
                    if let Some(cmd) = self.state.cancel_upload() {
                        (self.send)(cmd);
                    }
                }
                Step::Report => (self.send)(Cmd::Report),
                Step::Pretend(event) => self.take_event(event, drained),
            }
        }
    }

    fn report_ready(&mut self, data: &ReportData, drained: &mut Drained) {
        let outcome = self.session.report_ready(data, self.data_dir.as_deref());
        self.state.message = Some(outcome.note);
        drained.to_open.extend(outcome.open);
    }
}

/// What the worker says, on the console of a debug build. Frames are many, and progress is constant.
fn print_event(event: &Event) {
    match event {
        Event::ImageLoaded(image) => {
            eprintln!(
                "image: {} frames, {} sent, {} chunks each, {:?}",
                image.animation.frames().len(),
                image.plan.frame_count(),
                image.plan.chunks_per_frame,
                image.estimate
            );
        }
        Event::Progress { .. } => {}
        other => eprintln!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc::channel;

    use unkludge_protocol::model;

    use super::*;
    use crate::args::DemoArgs;
    use crate::demo::{DemoKeyboard, DemoState};
    use crate::state::Upload;
    use crate::worker::Problem;
    use unkludge_protocol::lighting::{CurrentLighting, LightMode, SleepSetting};

    struct Fixture {
        state: State,
        settings: Settings,
        session: Session,
        script: Option<Script>,
        sent: Vec<&'static str>,
    }

    fn fixture(script: Option<DemoState>) -> Fixture {
        let session = Session::Demo(DemoArgs::default());
        let script = script.map(|state| {
            let keyboard = DemoKeyboard::new(model::find(0x0265).unwrap(), state);
            Script::new(state, keyboard, None)
        });
        Fixture {
            state: State::default(),
            settings: Settings::default(),
            session,
            script,
            sent: Vec::new(),
        }
    }

    fn name(cmd: &Cmd) -> &'static str {
        match cmd {
            Cmd::Report => "report",
            Cmd::LoadImage { .. } => "load",
            Cmd::UploadScreen(_) => "upload",
            _ => "other",
        }
    }

    impl Fixture {
        fn drain(&mut self, events: Vec<Event>, data_dir: Option<&Path>) -> Drained {
            let (tx, rx) = channel();
            for event in events {
                tx.send(event).unwrap();
            }
            let sent = &mut self.sent;
            let mut send = |cmd: Cmd| sent.push(name(&cmd));
            Parts {
                state: &mut self.state,
                settings: &mut self.settings,
                session: &self.session,
                script: &mut self.script,
                data_dir: data_dir.map(Path::to_path_buf),
                send: &mut send,
            }
            .drain(&rx)
        }
    }

    fn lights() -> Event {
        Event::Lighting(CurrentLighting {
            mode: LightMode::Off,
            brightness: None,
            speed: None,
            color: None,
            sleep: SleepSetting::Unknown(0),
        })
    }

    fn progress() -> Event {
        Event::Progress {
            done: 1,
            total: 2,
            remaining: Duration::from_secs(1),
        }
    }

    fn connected() -> Event {
        Event::Connected {
            model: model::find(0x0265).unwrap(),
        }
    }

    #[test]
    fn with_nothing_waiting_nothing_changes() {
        let drained = fixture(None).drain(Vec::new(), None);
        assert_eq!(
            drained,
            Drained {
                only_progress: true,
                ..Drained::default()
            }
        );
    }

    #[test]
    fn progress_alone_can_wait_for_its_redraw() {
        let mut f = fixture(None);
        assert!(f.drain(vec![progress(), progress()], None).only_progress);
        assert!(!f.drain(vec![progress(), lights()], None).only_progress);
    }

    #[test]
    fn an_upload_ending_says_so_once() {
        let mut f = fixture(None);
        f.state.upload = Upload::Starting { cancelling: false };
        assert!(!f.drain(vec![progress()], None).upload_ended);
        assert!(f.drain(vec![Event::UploadDone], None).upload_ended);
        assert!(!f.drain(vec![lights()], None).upload_ended);
    }

    #[test]
    fn losing_the_keyboard_ends_an_upload_and_its_preview() {
        let mut f = fixture(None);
        f.state.upload = Upload::Starting { cancelling: false };
        let drained = f.drain(vec![Event::Disconnected], None);
        assert!(drained.upload_ended);
        assert!(drained.image_changed);
    }

    #[test]
    fn a_new_message_says_whether_it_fades() {
        let mut f = fixture(None);
        let synced = f.drain(vec![Event::ClockSynced], None);
        assert_eq!(synced.message_fades, Some(true));
        assert_eq!(
            f.drain(vec![Event::ClockSynced], None).message_fades,
            None,
            "the same one again is no news"
        );
        let problem = f.drain(vec![Event::Error(Problem::Busy)], None);
        assert_eq!(problem.message_fades, Some(false));
    }

    #[test]
    fn palettes_changing_ask_for_a_save() {
        let mut f = fixture(None);
        let drained = f.drain(
            vec![Event::PalettesChanged {
                pid: 0x0265,
                palettes: Box::default(),
            }],
            None,
        );
        assert!(drained.save_settings);
    }

    #[test]
    fn a_keyboard_arriving_loads_a_file_that_was_waiting() {
        let mut f = fixture(None);
        let _ = f.state.choose_file(PathBuf::from("a.gif"));
        f.drain(vec![connected()], None);
        assert_eq!(f.sent, ["load"]);
    }

    #[test]
    fn a_script_gets_to_see_events_and_its_steps_get_done() {
        let mut f = fixture(Some(DemoState::Report));
        f.drain(vec![connected(), lights()], None);
        assert_eq!(f.sent, ["report"]);
    }

    #[test]
    fn what_a_script_pretends_counts_as_having_happened() {
        let mut f = fixture(Some(DemoState::Hung));
        f.drain(vec![connected(), lights()], None);
        assert!(f.state.hung);
    }

    #[test]
    fn a_report_says_how_it_went_and_what_to_open() {
        let dir = std::env::temp_dir().join(format!("unkludge-drive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = fixture(None);
        let data = ReportData {
            model: None,
            config: Err("no".to_string()),
            errors: Vec::new(),
        };
        let drained = f.drain(vec![Event::ReportReady(data.clone())], Some(&dir));
        assert_eq!(drained.to_open, std::slice::from_ref(&dir));
        assert_eq!(drained.message_fades, Some(true));

        // With no folder to put it in, that's the news, and it stays.
        let drained = f.drain(vec![Event::ReportReady(data)], None);
        assert!(drained.to_open.is_empty());
        assert_eq!(drained.message_fades, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
