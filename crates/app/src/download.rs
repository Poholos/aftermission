//! Files the page hands the browser: an export saved as a download. The
//! bytes become a `Blob`, the blob an object URL, and the URL the target
//! of a link that is clicked from code; the browser then saves the file in
//! its downloads folder, or asks where, as its settings say.

/// The media type a download is labeled with, by its name's extension:
/// CSV as `text/csv`, so a spreadsheet offers to open it, and anything
/// else, such as a `.param` file, as plain text.
#[must_use]
pub fn media_type(name: &str) -> &'static str {
    let csv = std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    if csv {
        "text/csv;charset=utf-8"
    } else {
        "text/plain;charset=utf-8"
    }
}

/// Save `bytes` as a download named `name`.
///
/// # Errors
///
/// What the browser reports when it cannot make the blob, its URL or the
/// link, as text.
#[cfg(target_arch = "wasm32")]
pub fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use eframe::wasm_bindgen::JsCast as _;
    use eframe::wasm_bindgen::closure::Closure;

    let describe =
        |e: eframe::wasm_bindgen::JsValue| format!("the browser refused the download: {e:?}");
    let window = web_sys::window().ok_or("the page has no window")?;
    let document = window.document().ok_or("the page has no document")?;

    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(media_type(name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(describe)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(describe)?;

    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(describe)?
        .dyn_into()
        .map_err(|_element| "the page made no link".to_string())?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();

    // The browser reads the blob after the click returns, so the URL is let
    // go a minute later rather than at once, which some browsers answer
    // with a failed download.
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    if window
        .set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 60_000)
        .is_err()
    {
        tracing::debug!("cannot schedule the download URL's release; the page keeps it");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::media_type;

    #[test]
    fn a_csv_is_labeled_as_csv_and_anything_else_as_text() {
        assert_eq!(media_type("flight.csv"), "text/csv;charset=utf-8");
        assert_eq!(media_type("FLIGHT.CSV"), "text/csv;charset=utf-8");
        assert_eq!(media_type("flight_boot.param"), "text/plain;charset=utf-8");
        assert_eq!(media_type("csv"), "text/plain;charset=utf-8");
    }
}
