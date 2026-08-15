//! Appending to a `String` without pretending it can fail.
//!
//! The generators are one long sequence of `write!`/`writeln!` into a
//! `String`. Those return a `Result` because `fmt::Write` is general
//! over sinks that can fail -- a socket, a file. `String` is not one:
//! its `write_str` pushes onto a `Vec<u8>` and returns `Ok(())`,
//! always.
//!
//! So the `.unwrap()` at each of those sites was a branch that could
//! never be taken, and there were 78 of them. The cost is not the
//! characters; it is that a reader auditing where this crate can panic
//! has to visit all 78 and reach the same conclusion each time, which
//! is exactly the kind of noise that hides the one unwrap that
//! genuinely can fire.
//!
//! These assert it once, here, and read as what they are: appending a
//! line.

/// `writeln!` into a `String`, which cannot fail.
macro_rules! wln {
    ($out:expr $(, $($arg:tt)*)?) => {{
        use std::fmt::Write as _;
        let _ = writeln!($out $(, $($arg)*)?);
    }};
}

/// `write!` into a `String`, which cannot fail.
macro_rules! w {
    ($out:expr $(, $($arg:tt)*)?) => {{
        use std::fmt::Write as _;
        let _ = write!($out $(, $($arg)*)?);
    }};
}

pub(crate) use {w, wln};
