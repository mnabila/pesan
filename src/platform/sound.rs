/// Sound to play with a desktop notification, mapped to a freedesktop hint by
/// the notifier adapter. `File` is an absolute path to a sound file (`sound-file`
/// hint); `Name` is a sound-theme name like `message-new-instant` (`sound-name`
/// hint). Lives in `platform` because both `config` (which parses it) and the
/// mail `Notifier` port (which consumes it) reference it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundHint {
    Name(String),
    File(String),
}
