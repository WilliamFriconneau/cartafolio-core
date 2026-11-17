//! Where the commands write: the terminal, or, when the node runs a command for a remote caller, a buffer.
//!
//! Every command prints with `say!`, `sayln!`, `esay!` and `esayln!` (the same as `print!`, `println!`, `eprint!` and
//! `eprintln!`), so that the node can run the very same function for a caller over HTTP and hand back exactly the text
//! the local command line would have printed. A thread that [`capture`]s has its own buffer; any other thread writes to
//! the terminal.

use std::cell::RefCell;
use std::fmt::Arguments;
use std::io::Write;

type Buffers = (String, String);

thread_local! {
    static SINK: RefCell<Option<Buffers>> = const { RefCell::new(None) };
}

/// Writes to standard output (`error` false) or standard error, or to the buffer of this thread.
pub fn emit(error: bool, args: Arguments) {
    let taken = SINK.with(|s| {
        if let Some((o, e)) = s.borrow_mut().as_mut() {
            let text = std::fmt::format(args);
            if error {
                e.push_str(&text);
            } else {
                o.push_str(&text);
            }
            true
        } else {
            false
        }
    });
    if !taken {
        if error {
            let _ = write!(std::io::stderr(), "{args}");
        } else {
            let _ = write!(std::io::stdout(), "{args}");
        }
    }
}

/// Runs `f` with what it prints kept: `(its result, standard output, standard error)`.
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, String, String) {
    let before = SINK.with(|s| s.borrow_mut().replace((String::new(), String::new())));
    let result = f();
    let (o, e) = SINK
        .with(|s| std::mem::replace(&mut *s.borrow_mut(), before))
        .unwrap_or_default();
    (result, o, e)
}

#[macro_export]
macro_rules! say {
    ($($a:tt)*) => { $crate::out::emit(false, format_args!($($a)*)) };
}
#[macro_export]
macro_rules! sayln {
    () => { $crate::out::emit(false, format_args!("\n")) };
    ($($a:tt)*) => { $crate::out::emit(false, format_args!("{}\n", format_args!($($a)*))) };
}
#[macro_export]
macro_rules! esay {
    ($($a:tt)*) => { $crate::out::emit(true, format_args!($($a)*)) };
}
#[macro_export]
macro_rules! esayln {
    () => { $crate::out::emit(true, format_args!("\n")) };
    ($($a:tt)*) => { $crate::out::emit(true, format_args!("{}\n", format_args!($($a)*))) };
}
