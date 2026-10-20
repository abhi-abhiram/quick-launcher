//! The check shared by every type that takes exactly one of several forms:
//! `run`, icon objects and response bodies.

/// Returns the only form given, or an error naming `what` and its `keys`
/// that says whether none or several were given.
pub(crate) fn exactly_one<T>(mut forms: Vec<T>, what: &str, keys: &str) -> Result<T, String> {
    match forms.len() {
        1 => Ok(forms.remove(0)),
        0 => Err(format!("{what} needs one of {keys}")),
        _ => Err(format!("{what} must have only one of {keys}")),
    }
}
