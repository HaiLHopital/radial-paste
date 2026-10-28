use std::time::Instant;

use radial_domain::{Config, HotkeyBinding, Vec2};
use thiserror::Error;

use crate::ports::{HotkeyRegistrar, MenuPresenter, TextInjector};

/// What the input loop and the settings window tell the [`Core`].
#[derive(Clone, Debug)]
pub enum Event {
    /// The hotkey went down with the mouse at `cursor`.
    HotkeyPressed {
        cursor: Vec2,
    },
    /// Sampled every few milliseconds while [`Core::wants_ticks`] is true.
    Tick {
        now: Instant,
        cursor: Vec2,
        /// The hotkey's main key is still held.
        key_down: bool,
        /// Any of the hotkey's modifiers is still held.
        modifiers_down: bool,
    },
    /// A mouse button went down at `cursor`, on the menu or anywhere else.
    Click {
        now: Instant,
        cursor: Vec2,
    },
    /// Esc, or anything else that should close the menu without pasting.
    Cancel,
    SetPaused(bool),
    /// New settings were saved.
    ConfigChanged(Config),
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("could not paste: {0}")]
    Paste(String),
    #[error("could not register the hotkey {hotkey}: {reason}")]
    Hotkey { hotkey: String, reason: String },
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Idle,
    /// The hotkey is held: letting go over a wedge picks it.
    Holding {
        origin: Vec2,
        scale: f64,
        highlighted: Option<usize>,
    },
    /// The hotkey was let go in the dead zone: the menu waits for a click.
    Open {
        origin: Vec2,
        scale: f64,
        highlighted: Option<usize>,
    },
    /// A snippet was chosen and the menu is gone. Waiting for the hotkey's modifiers to come up,
    /// so that the paste is not read as, say, Ctrl+Shift+V.
    Releasing { text: String, deadline: Instant },
}

/// The menu's state machine: hold the hotkey and flick toward a wedge, or tap it and click.
pub struct Core<M, I, H> {
    config: Config,
    paused: bool,
    state: State,
    menu: M,
    injector: I,
    hotkeys: H,
}

impl<M: MenuPresenter, I: TextInjector, H: HotkeyRegistrar> Core<M, I, H> {
    pub fn new(config: Config, menu: M, injector: I, hotkeys: H) -> Self {
        Self {
            config,
            paused: false,
            state: State::Idle,
            menu,
            injector,
            hotkeys,
        }
    }

    /// Registers the configured hotkey. Call it on the thread that runs the input loop.
    pub fn start(&mut self) -> Result<(), CoreError> {
        let hotkey = self.config.hotkey;
        self.hotkeys
            .register(hotkey)
            .map_err(|reason| hotkey_error(hotkey, reason))
    }

    /// The settings in effect. After a failed hotkey change this keeps the old hotkey.
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The hotkey registrar, for the input loop to register and release Esc with the menu.
    pub fn hotkeys_mut(&mut self) -> &mut H {
        &mut self.hotkeys
    }

    /// Whether the input loop should be sending [`Event::Tick`]s.
    pub fn wants_ticks(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    pub fn handle(&mut self, event: Event) -> Result<(), CoreError> {
        match event {
            Event::HotkeyPressed { cursor } => self.on_hotkey(cursor),
            Event::Tick {
                now,
                cursor,
                key_down,
                modifiers_down,
            } => return self.on_tick(now, cursor, key_down, modifiers_down),
            Event::Click { now, cursor } => self.on_click(now, cursor),
            Event::Cancel => self.close(),
            Event::SetPaused(paused) => {
                if paused {
                    self.close();
                }
                self.paused = paused;
            }
            Event::ConfigChanged(config) => return self.on_config(config),
        }
        Ok(())
    }

    fn on_hotkey(&mut self, cursor: Vec2) {
        match self.state {
            State::Idle if !self.paused && !self.config.snippets.is_empty() => {
                let placement = self
                    .menu
                    .show(cursor, &self.config.snippets, self.config.layout);
                self.state = State::Holding {
                    origin: placement.centre,
                    scale: placement.scale,
                    highlighted: None,
                };
            }
            // Pressing the hotkey again closes a menu that stayed open.
            State::Open { .. } => self.close(),
            _ => {}
        }
    }

    fn on_tick(
        &mut self,
        now: Instant,
        cursor: Vec2,
        key_down: bool,
        modifiers_down: bool,
    ) -> Result<(), CoreError> {
        match std::mem::take(&mut self.state) {
            State::Idle => {}
            State::Holding {
                origin,
                scale,
                highlighted,
            } => {
                let wedge = self.wedge_at((cursor - origin) / scale);
                if key_down {
                    self.set_highlight(highlighted, wedge);
                    self.state = State::Holding {
                        origin,
                        scale,
                        highlighted: wedge,
                    };
                } else if let Some(index) = wedge {
                    self.choose(index, now);
                } else {
                    // Let go without a flick: keep the menu up for a click.
                    self.set_highlight(highlighted, None);
                    self.state = State::Open {
                        origin,
                        scale,
                        highlighted: None,
                    };
                }
            }
            State::Open {
                origin,
                scale,
                highlighted,
            } => {
                let offset = (cursor - origin) / scale;
                let wedge = if self.config.layout.contains(offset) {
                    self.wedge_at(offset)
                } else {
                    None
                };
                self.set_highlight(highlighted, wedge);
                self.state = State::Open {
                    origin,
                    scale,
                    highlighted: wedge,
                };
            }
            State::Releasing { text, deadline } => {
                if modifiers_down && now < deadline {
                    self.state = State::Releasing { text, deadline };
                } else {
                    return self
                        .injector
                        .paste(&text)
                        .map_err(|e| CoreError::Paste(e.to_string()));
                }
            }
        }
        Ok(())
    }

    fn on_click(&mut self, now: Instant, cursor: Vec2) {
        let State::Open { origin, scale, .. } = self.state else {
            return;
        };
        let offset = (cursor - origin) / scale;
        match self.wedge_at(offset) {
            Some(index) if self.config.layout.contains(offset) => self.choose(index, now),
            // The centre, or off the menu.
            _ => self.close(),
        }
    }

    fn on_config(&mut self, mut config: Config) -> Result<(), CoreError> {
        self.close();
        let old = self.config.hotkey;
        let mut result = Ok(());
        if config.hotkey != old
            && let Err(reason) = self.hotkeys.register(config.hotkey)
        {
            result = Err(hotkey_error(config.hotkey, reason));
            config.hotkey = old;
        }
        self.config = config;
        result
    }

    fn wedge_at(&self, offset: Vec2) -> Option<usize> {
        self.config.layout.pick(self.config.snippets.len(), offset)
    }

    fn set_highlight(&mut self, old: Option<usize>, new: Option<usize>) {
        if old != new {
            self.menu.highlight(new);
        }
    }

    fn choose(&mut self, index: usize, now: Instant) {
        self.menu.hide();
        self.state = match self.config.snippets.get(index) {
            Some(snippet) => State::Releasing {
                text: snippet.text.clone(),
                deadline: now + self.config.modifier_release_timeout(),
            },
            None => State::Idle,
        };
    }

    fn close(&mut self) {
        if matches!(self.state, State::Holding { .. } | State::Open { .. }) {
            self.menu.hide();
        }
        self.state = State::Idle;
    }
}

fn hotkey_error(hotkey: HotkeyBinding, reason: impl std::fmt::Display) -> CoreError {
    CoreError::Hotkey {
        hotkey: hotkey.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use radial_domain::{Layout, Snippet, Vec2};

    use super::*;
    use crate::ports::Placement;

    #[derive(Debug, PartialEq)]
    enum Call {
        Show(Vec2),
        Highlight(Option<usize>),
        Hide,
        Paste(String),
        Register(String),
    }

    type Log = Rc<RefCell<Vec<Call>>>;

    /// Moves the menu `shift` pixels right of the cursor, as if near the left screen edge, and
    /// reports the given monitor `scale`.
    struct FakeMenu {
        log: Log,
        shift: f64,
        scale: f64,
    }

    impl MenuPresenter for FakeMenu {
        fn show(&mut self, cursor: Vec2, _: &[Snippet], _: Layout) -> Placement {
            self.log.borrow_mut().push(Call::Show(cursor));
            Placement {
                centre: Vec2::new(cursor.x + self.shift, cursor.y),
                scale: self.scale,
            }
        }
        fn highlight(&mut self, wedge: Option<usize>) {
            self.log.borrow_mut().push(Call::Highlight(wedge));
        }
        fn hide(&mut self) {
            self.log.borrow_mut().push(Call::Hide);
        }
    }

    struct FakeInjector(Log);

    impl TextInjector for FakeInjector {
        type Error = String;
        fn paste(&mut self, text: &str) -> Result<(), String> {
            self.0.borrow_mut().push(Call::Paste(text.to_owned()));
            Ok(())
        }
    }

    struct FakeHotkeys {
        log: Log,
        fail: bool,
    }

    impl HotkeyRegistrar for FakeHotkeys {
        type Error = &'static str;
        fn register(&mut self, binding: HotkeyBinding) -> Result<(), &'static str> {
            self.log
                .borrow_mut()
                .push(Call::Register(binding.to_string()));
            if self.fail { Err("taken") } else { Ok(()) }
        }
    }

    type TestCore = Core<FakeMenu, FakeInjector, FakeHotkeys>;

    const ORIGIN: Vec2 = Vec2::new(500.0, 500.0);
    const UP: Vec2 = Vec2::new(500.0, 400.0);
    const RIGHT: Vec2 = Vec2::new(600.0, 500.0);
    const CENTRE: Vec2 = Vec2::new(505.0, 505.0);
    const FAR_RIGHT: Vec2 = Vec2::new(900.0, 500.0);

    fn config() -> Config {
        Config {
            layout: Layout {
                dead_zone: 20.0,
                radius: 140.0,
            },
            modifier_release_timeout_ms: 300,
            snippets: ["a", "b", "c", "d"]
                .map(|s| Snippet::new(s.to_uppercase(), format!("text {s}")))
                .to_vec(),
            ..Config::default()
        }
    }

    fn core_with(config: Config, shift: f64, scale: f64, fail_hotkeys: bool) -> (TestCore, Log) {
        let log = Log::default();
        let core = Core::new(
            config,
            FakeMenu {
                log: log.clone(),
                shift,
                scale,
            },
            FakeInjector(log.clone()),
            FakeHotkeys {
                log: log.clone(),
                fail: fail_hotkeys,
            },
        );
        (core, log)
    }

    fn core() -> (TestCore, Log) {
        core_with(config(), 0.0, 1.0, false)
    }

    fn tick(now: Instant, cursor: Vec2, key_down: bool, modifiers_down: bool) -> Event {
        Event::Tick {
            now,
            cursor,
            key_down,
            modifiers_down,
        }
    }

    fn press(core: &mut TestCore) {
        core.handle(Event::HotkeyPressed { cursor: ORIGIN })
            .unwrap();
    }

    #[test]
    fn flick_pastes_once_the_modifiers_are_up() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, UP, true, true)).unwrap();
        core.handle(tick(t, UP, true, true)).unwrap();
        core.handle(tick(t, UP, false, true)).unwrap();
        core.handle(tick(t + Duration::from_millis(10), UP, false, true))
            .unwrap();
        assert!(core.wants_ticks());
        core.handle(tick(t + Duration::from_millis(20), UP, false, false))
            .unwrap();

        assert_eq!(
            *log.borrow(),
            [
                Call::Show(ORIGIN),
                Call::Highlight(Some(0)),
                Call::Hide,
                Call::Paste("text a".into()),
            ]
        );
        assert!(!core.wants_ticks());
    }

    #[test]
    fn modifiers_held_past_the_timeout_paste_anyway() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, RIGHT, false, true)).unwrap();
        core.handle(tick(t + Duration::from_millis(300), RIGHT, false, true))
            .unwrap();
        assert_eq!(log.borrow().last(), Some(&Call::Paste("text b".into())));
    }

    #[test]
    fn flick_far_past_the_ring_still_picks() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, FAR_RIGHT, false, false)).unwrap();
        core.handle(tick(t, FAR_RIGHT, false, false)).unwrap();
        assert_eq!(log.borrow().last(), Some(&Call::Paste("text b".into())));
    }

    #[test]
    fn tap_keeps_the_menu_open_for_a_click() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, CENTRE, false, false)).unwrap();
        core.handle(tick(t, RIGHT, false, false)).unwrap();
        core.handle(Event::Click {
            now: t,
            cursor: RIGHT,
        })
        .unwrap();
        core.handle(tick(t, RIGHT, false, false)).unwrap();

        assert_eq!(
            *log.borrow(),
            [
                Call::Show(ORIGIN),
                Call::Highlight(Some(1)),
                Call::Hide,
                Call::Paste("text b".into()),
            ]
        );
    }

    #[test]
    fn leaving_the_ring_clears_the_highlight_of_an_open_menu() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, CENTRE, false, false)).unwrap();
        core.handle(tick(t, RIGHT, false, false)).unwrap();
        core.handle(tick(t, FAR_RIGHT, false, false)).unwrap();
        assert_eq!(log.borrow().last(), Some(&Call::Highlight(None)));
    }

    #[test]
    fn clicks_in_the_centre_or_off_the_menu_cancel() {
        for cursor in [CENTRE, FAR_RIGHT] {
            let (mut core, log) = core();
            let t = Instant::now();
            press(&mut core);
            core.handle(tick(t, CENTRE, false, false)).unwrap();
            core.handle(Event::Click { now: t, cursor }).unwrap();
            assert_eq!(*log.borrow(), [Call::Show(ORIGIN), Call::Hide]);
            assert!(!core.wants_ticks());
        }
    }

    #[test]
    fn hotkey_again_or_cancel_closes_the_menu() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, CENTRE, false, false)).unwrap();
        press(&mut core);
        assert_eq!(*log.borrow(), [Call::Show(ORIGIN), Call::Hide]);

        let (mut other, log) = self::core();
        press(&mut other);
        other.handle(Event::Cancel).unwrap();
        assert_eq!(*log.borrow(), [Call::Show(ORIGIN), Call::Hide]);
    }

    #[test]
    fn cancel_while_releasing_drops_the_paste_without_hiding_twice() {
        let (mut core, log) = core();
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, UP, false, true)).unwrap();
        core.handle(Event::Cancel).unwrap();
        core.handle(tick(t, UP, false, false)).unwrap();
        assert_eq!(*log.borrow(), [Call::Show(ORIGIN), Call::Hide]);
    }

    #[test]
    fn picks_from_the_centre_the_menu_used() {
        // The menu moved 100 px right, so the cursor at ORIGIN is now left of its centre.
        let (mut core, log) = core_with(config(), 100.0, 1.0, false);
        let t = Instant::now();
        press(&mut core);
        core.handle(tick(t, ORIGIN, false, false)).unwrap();
        core.handle(tick(t, ORIGIN, false, false)).unwrap();
        assert_eq!(log.borrow().last(), Some(&Call::Paste("text d".into())));
    }

    #[test]
    fn a_hidpi_monitor_divides_the_offsets() {
        // Two hundred physical pixels right are one hundred logical ones: wedge 1, not a flick
        // far past the ring.
        let (mut core, log) = core_with(config(), 0.0, 2.0, false);
        let t = Instant::now();
        press(&mut core);
        let cursor = Vec2::new(ORIGIN.x + 200.0, ORIGIN.y);
        core.handle(tick(t, cursor, true, true)).unwrap();
        core.handle(tick(t, cursor, false, true)).unwrap();
        core.handle(tick(t, cursor, false, false)).unwrap();
        assert_eq!(log.borrow().last(), Some(&Call::Paste("text b".into())));
    }

    #[test]
    fn paused_or_empty_ignores_the_hotkey() {
        let (mut core, log) = core();
        core.handle(Event::SetPaused(true)).unwrap();
        press(&mut core);
        assert!(log.borrow().is_empty());
        core.handle(Event::SetPaused(false)).unwrap();
        press(&mut core);
        assert_eq!(*log.borrow(), [Call::Show(ORIGIN)]);

        let empty = Config {
            snippets: Vec::new(),
            ..config()
        };
        let (mut core, log) = core_with(empty, 0.0, 1.0, false);
        press(&mut core);
        assert!(log.borrow().is_empty());
        assert!(!core.wants_ticks());
    }

    #[test]
    fn config_change_closes_the_menu_and_registers_a_new_hotkey() {
        let (mut core, log) = core();
        core.start().unwrap();
        press(&mut core);
        let new = Config {
            hotkey: "Ctrl+Alt+Q".parse().unwrap(),
            ..config()
        };
        core.handle(Event::ConfigChanged(new.clone())).unwrap();
        core.handle(Event::ConfigChanged(new)).unwrap();
        assert_eq!(
            *log.borrow(),
            [
                Call::Register("Ctrl+Shift+Space".into()),
                Call::Show(ORIGIN),
                Call::Hide,
                Call::Register("Ctrl+Alt+Q".into()),
            ]
        );
    }

    #[test]
    fn failed_hotkey_change_keeps_the_old_hotkey_and_the_rest() {
        let (mut core, _) = core_with(config(), 0.0, 1.0, true);
        let new = Config {
            hotkey: "Ctrl+Alt+Q".parse().unwrap(),
            snippets: vec![Snippet::new("Z", "text z")],
            ..config()
        };
        let err = core.handle(Event::ConfigChanged(new)).unwrap_err();
        assert_eq!(
            err,
            CoreError::Hotkey {
                hotkey: "Ctrl+Alt+Q".into(),
                reason: "taken".into()
            }
        );
        assert_eq!(core.config().hotkey.to_string(), "Ctrl+Shift+Space");
        assert_eq!(core.config().snippets.len(), 1);
    }
}
