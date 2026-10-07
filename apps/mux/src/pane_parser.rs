//! A pane's output parsed on a thread of its own, so a flood of output never
//! holds up typing or drawing on the main thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;

use mux_terminal::{TerminalEngine, TerminalError};
use mux_terminal_ghostty::GhosttyEngine;
use parking_lot::{Mutex, MutexGuard};

/// A pane's terminal, shared by the main thread and the pane's parser.
pub type SharedEngine = Arc<Mutex<GhosttyEngine>>;

/// Output this small is parsed where it arrives when nothing is waiting
/// ahead of it, so an echo is drawn without a trip through another thread.
pub const PARSE_IN_PLACE_BYTES: usize = 4 * 1024;

/// How much the parser takes in before the window draws what it has, so a
/// long flood still moves on screen.
const PARSED_BETWEEN_DRAWS: usize = 256 * 1024;

/// How much the parser takes in at a time before letting the window at the
/// terminal, so the window never waits long to draw it.
const PARSED_PER_TURN: usize = 16 * 1024;

/// What the parser says when it has taken in output: `None`, or what went
/// wrong.
pub type Notify = Box<dyn Fn(Option<String>) + Send>;

/// The parser of one pane's output: what it is handed is applied in order,
/// and it says so, once until heard, whenever the window should draw again.
pub struct OutputParser {
    queue: Sender<(u64, Vec<u8>)>,
    unheard: Arc<AtomicBool>,
}

impl OutputParser {
    pub fn spawn(name: String, engine: SharedEngine, notify: Notify) -> std::io::Result<Self> {
        let (queue, receiver) = channel();
        let unheard = Arc::new(AtomicBool::new(false));
        let parser_unheard = Arc::clone(&unheard);
        thread::Builder::new()
            .name(name)
            .spawn(move || parse(&receiver, &engine, &parser_unheard, &notify))?;
        Ok(Self { queue, unheard })
    }

    /// Hands over output to apply after everything handed over before it.
    pub fn send(&self, sequence: u64, bytes: Vec<u8>) {
        // The thread only stops once this parser is dropped.
        let _ = self.queue.send((sequence, bytes));
    }

    /// The window has heard the parser: the next output it takes in is
    /// worth saying again.
    pub fn heard(&self) {
        self.unheard.store(false, Ordering::Release);
    }
}

fn parse(
    receiver: &Receiver<(u64, Vec<u8>)>,
    engine: &Mutex<GhosttyEngine>,
    unheard: &AtomicBool,
    notify: &Notify,
) {
    let say_parsed = || {
        if !unheard.swap(true, Ordering::AcqRel) {
            notify(None);
        }
    };
    let mut since_draw = 0;
    let mut next = receiver.recv().ok();
    while let Some((sequence, bytes)) = next {
        if let Err(error) = apply(engine, sequence, &bytes) {
            notify(Some(error.to_string()));
        }
        since_draw += bytes.len();
        next = match receiver.try_recv() {
            Ok(more) => {
                if since_draw >= PARSED_BETWEEN_DRAWS {
                    since_draw = 0;
                    say_parsed();
                }
                Some(more)
            }
            Err(TryRecvError::Empty) => {
                since_draw = 0;
                say_parsed();
                receiver.recv().ok()
            }
            Err(TryRecvError::Disconnected) => None,
        };
    }
}

/// Applies one output a part at a time, handing the terminal straight to
/// the window between parts if it is waiting, so it waits for one part at
/// most, however long the output.
fn apply(
    engine: &Mutex<GhosttyEngine>,
    sequence: u64,
    mut bytes: &[u8],
) -> Result<(), TerminalError> {
    loop {
        let last = bytes.len() <= PARSED_PER_TURN;
        let (part, rest) = bytes.split_at(bytes.len().min(PARSED_PER_TURN));
        let mut parsing = engine.lock();
        let applied = if last {
            parsing.apply_output(sequence, part)
        } else {
            parsing.apply_output_part(sequence, part)
        };
        MutexGuard::unlock_fair(parsing);
        if last || applied.is_err() {
            return applied;
        }
        bytes = rest;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::Duration;

    use mux_terminal::{TerminalEngine, TerminalSize};

    use super::*;

    fn engine() -> SharedEngine {
        let engine = GhosttyEngine::new(TerminalSize {
            cols: 40,
            rows: 4,
            ..TerminalSize::default()
        })
        .expect("new terminal");
        Arc::new(Mutex::new(engine))
    }

    fn text(engine: &SharedEngine) -> String {
        engine.lock().screen_text().expect("screen text")
    }

    #[test]
    fn output_is_applied_in_order_and_said_once_until_heard() {
        let engine = engine();
        let (said, heard) = channel();
        let parser = OutputParser::spawn(
            "test-parser".into(),
            Arc::clone(&engine),
            Box::new(move |error| {
                let _ = said.send(error);
            }),
        )
        .expect("parser");

        parser.send(1, b"one ".to_vec());
        parser.send(2, b"two".to_vec());
        assert_eq!(heard.recv_timeout(Duration::from_secs(10)), Ok(None));
        while engine.lock().next_output_sequence() < 3 {
            assert_eq!(
                heard.recv_timeout(Duration::from_millis(50)),
                Err(RecvTimeoutError::Timeout),
                "said again before it was heard"
            );
        }
        assert!(text(&engine).contains("one two"));

        parser.heard();
        parser.send(3, b" three".to_vec());
        assert_eq!(heard.recv_timeout(Duration::from_secs(10)), Ok(None));
        assert!(text(&engine).contains("one two three"));
    }

    #[test]
    fn output_out_of_order_is_said_as_an_error() {
        let engine = engine();
        let (said, heard) = channel();
        let parser = OutputParser::spawn(
            "test-parser".into(),
            Arc::clone(&engine),
            Box::new(move |error| {
                let _ = said.send(error);
            }),
        )
        .expect("parser");

        parser.send(2, b"early".to_vec());

        let error = heard.recv_timeout(Duration::from_secs(10)).expect("said");
        assert!(error.is_some());
        assert_eq!(engine.lock().next_output_sequence(), 1);
    }

    #[test]
    fn a_long_output_is_applied_whole_a_part_at_a_time() {
        let engine = engine();
        let mut long = b"x".repeat(PARSED_PER_TURN * 3 + 7);
        long.extend_from_slice(b"\r\nlast line");

        apply(&engine, 1, &long).expect("applied");
        assert_eq!(engine.lock().next_output_sequence(), 2);
        assert!(text(&engine).contains("last line"));

        let error = apply(&engine, 1, &long).expect_err("already applied");
        assert!(matches!(error, TerminalError::OutOfOrder { .. }));
        assert_eq!(engine.lock().next_output_sequence(), 2);
    }
}
