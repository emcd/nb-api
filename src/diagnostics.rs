//! Output presentation helpers.

pub(crate) fn append_warning(mut output: String, warning: String) -> String {
    if !output.trim().is_empty() {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str(&warning);
    output
}
