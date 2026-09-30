use crate::mail::Folder;

/// Carry over known counts by name onto freshly listed folders. A live connect
/// lists folder names only (counts come later in the background STATUS sweep),
/// so this avoids the sidebar blinking to zero before that sweep lands.
pub fn carry_counts(prev: &[Folder], folders: &mut [Folder]) {
    for f in folders.iter_mut() {
        if f.total == 0
            && f.unread == 0
            && let Some(p) = prev.iter().find(|p| p.name == f.name)
        {
            f.total = p.total;
            f.unread = p.unread;
        }
    }
}
