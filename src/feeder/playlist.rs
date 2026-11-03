use anyhow::{Result, bail};
use crossbeam_channel::Sender;
use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::sync::Mutex;

static ORIG_TERMIOS: Mutex<Option<nix::sys::termios::Termios>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq)]
pub enum StreamControl {
    Next,
    Prev,
    SeekDelta(f64),
    VolumeDelta(i32),
    TogglePause,
    ToggleLoop,
    ToggleCamera,
    ToggleMic,
    Stop,
}

/// Resolve command-line paths and optional playlist text files into a clean list of media files.
/// Resolves relative paths relative to the playlist file's directory if applicable.
pub fn resolve_playlist(
    files: &[PathBuf],
    playlist_file: Option<&PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();

    if let Some(pl) = playlist_file {
        if pl.exists() {
            let parent_dir = pl.parent();
            let content = std::fs::read_to_string(pl)?;
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    let mut p = PathBuf::from(trimmed);
                    if !p.exists()
                        && let Some(parent) = parent_dir
                    {
                        let rel = parent.join(trimmed);
                        if rel.exists() {
                            p = rel;
                        }
                    }
                    if p.exists() {
                        resolved.push(p);
                    } else {
                        eprintln!("Warning: playlist entry does not exist: {}", trimmed);
                    }
                }
            }
        } else {
            bail!("Playlist file not found: {}", pl.display());
        }
    }

    for f in files {
        if f.exists() {
            resolved.push(f.clone());
        } else {
            eprintln!("Warning: file does not exist: {}", f.display());
        }
    }

    if resolved.is_empty() {
        bail!(
            "No valid media files found to stream. Please specify existing files or a valid playlist."
        );
    }

    Ok(resolved)
}

/// Enable non-canonical raw mode on stdin for single-key interactive hotkeys.
/// Saves the original termios in a global static so it can be restored on Ctrl+C or process exit.
pub fn enable_raw_mode() {
    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    if nix::unistd::isatty(&stdin).unwrap_or(false)
        && let Ok(attr) = tcgetattr(&stdin)
    {
        let mut guard = ORIG_TERMIOS.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(attr.clone());
        }
        let mut raw = attr;
        raw.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO);
        let _ = tcsetattr(&stdin, SetArg::TCSANOW, &raw);
    }
}

/// Restore original terminal mode (re-enabling ICANON and ECHO).
/// Safe to call multiple times, from signal handlers, or on exit.
pub fn restore_terminal() {
    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    let mut restored = false;

    if let Ok(mut guard) = ORIG_TERMIOS.lock() {
        if let Some(ref orig) = *guard {
            let _ = tcsetattr(&stdin, SetArg::TCSANOW, orig);
            restored = true;
        }
        *guard = None;
    }

    // Fallback: If ORIG_TERMIOS was already consumed or unavailable, directly ensure ICANON & ECHO are restored
    if !restored
        && nix::unistd::isatty(&stdin).unwrap_or(false)
        && let Ok(mut attr) = tcgetattr(&stdin)
    {
        attr.local_flags
            .insert(LocalFlags::ICANON | LocalFlags::ECHO);
        let _ = tcsetattr(&stdin, SetArg::TCSANOW, &attr);
    }
}

/// Spawn a background thread listening for interactive hotkeys from stdin.
/// Enables raw non-canonical mode if stdin is an interactive TTY for instant single-key controls.
pub fn spawn_hotkey_listener(tx: Sender<StreamControl>) {
    enable_raw_mode();

    std::thread::Builder::new()
        .name("hotkey-listener".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            let is_tty = nix::unistd::isatty(&stdin).unwrap_or(false);

            if is_tty {
                let mut buf = [0u8; 1];
                let mut handle = stdin.lock();

                while handle.read_exact(&mut buf).is_ok() {
                    let ch = buf[0] as char;
                    let handled = match ch {
                        'n' | 'N' => tx.send(StreamControl::Next),
                        'p' | 'P' => tx.send(StreamControl::Prev),
                        'l' | 'L' => tx.send(StreamControl::ToggleLoop),
                        '+' | '=' | 'u' | 'U' => tx.send(StreamControl::VolumeDelta(5)),
                        '-' | '_' | 'd' | 'D' => tx.send(StreamControl::VolumeDelta(-5)),
                        'f' | 'F' | '>' => tx.send(StreamControl::SeekDelta(10.0)),
                        'b' | 'B' | '<' => tx.send(StreamControl::SeekDelta(-10.0)),
                        ' ' | 'k' | 'K' => tx.send(StreamControl::TogglePause),
                        'c' | 'C' => tx.send(StreamControl::ToggleCamera),
                        'm' | 'M' => tx.send(StreamControl::ToggleMic),
                        'q' | 'Q' => {
                            restore_terminal();
                            let _ = tx.send(StreamControl::Stop);
                            break;
                        }
                        '\x03' => {
                            // Ctrl+C in raw mode
                            restore_terminal();
                            let _ = tx.send(StreamControl::Stop);
                            break;
                        }
                        _ => Ok(()),
                    };
                    if handled.is_err() {
                        break;
                    }
                }
                restore_terminal();
            } else {
                // Non-TTY fallback (e.g. piped input or CI tests)
                for line in stdin.lock().lines().map_while(Result::ok) {
                    let cmd = line.trim().to_lowercase();
                    let handled = match cmd.as_str() {
                        "n" => tx.send(StreamControl::Next),
                        "p" => tx.send(StreamControl::Prev),
                        "l" => tx.send(StreamControl::ToggleLoop),
                        "+" | "u" | "vol+" => tx.send(StreamControl::VolumeDelta(5)),
                        "-" | "d" | "vol-" => tx.send(StreamControl::VolumeDelta(-5)),
                        "f" | ">" | "+10" => tx.send(StreamControl::SeekDelta(10.0)),
                        "b" | "<" | "-10" => tx.send(StreamControl::SeekDelta(-10.0)),
                        " " | "k" | "pause" => tx.send(StreamControl::TogglePause),
                        "c" => tx.send(StreamControl::ToggleCamera),
                        "m" => tx.send(StreamControl::ToggleMic),
                        "q" | "exit" | "quit" => {
                            let _ = tx.send(StreamControl::Stop);
                            break;
                        }
                        _ => Ok(()),
                    };
                    if handled.is_err() {
                        break;
                    }
                }
            }
        })
        .expect("Failed to spawn hotkey listener thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_playlist_empty() {
        let res = resolve_playlist(&[], None);
        assert!(res.is_err());
    }

    #[test]
    fn test_resolve_playlist_from_temp_file() {
        let temp_dir = std::env::temp_dir();
        let test_media = temp_dir.join("test_media_track.raw");
        std::fs::write(&test_media, b"fake media content").unwrap();

        let pl_path = temp_dir.join("test_playlist.txt");
        let content = format!("# Comment\n\n{}\n# Another comment\n", test_media.display());
        std::fs::write(&pl_path, content).unwrap();

        let resolved = resolve_playlist(&[], Some(&pl_path)).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0], test_media);

        let _ = std::fs::remove_file(test_media);
        let _ = std::fs::remove_file(pl_path);
    }

    #[test]
    fn test_terminal_restore_idempotent() {
        // Calling enable_raw_mode and restore_terminal should cycle cleanly
        enable_raw_mode();
        restore_terminal();
        // Calling restore_terminal multiple times should never fail or panic
        restore_terminal();
    }
}
