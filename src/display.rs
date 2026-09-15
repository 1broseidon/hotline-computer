//! What the agent has once the display is up: the screen as a stream for
//! viewers, the hands that press keys and buttons, and the clipboard.

use std::sync::Mutex;

use crate::clipboard::Clipboard;
use crate::screen::Screen;
use crate::xtest::Hands;

pub struct Display {
    pub screen: Screen,
    pub hands: Mutex<Hands>,
    pub clipboard: Clipboard,
    /// What the hands do with the pointer, for viewers that draw it.
    pub gestures: tokio::sync::broadcast::Sender<crate::xtest::Gesture>,
}

impl Display {
    pub fn open(display: &str) -> Result<Self, String> {
        let hands = Hands::new(display)?;
        Ok(Self {
            screen: Screen::start(display)?,
            gestures: hands.gestures(),
            hands: Mutex::new(hands),
            clipboard: Clipboard::start(display)?,
        })
    }
}
