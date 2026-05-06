//! Catalog row icons: embedded assets first, then HTTP (same resolution as core).

use appimage_control_core::{Catalog, CatalogApp};

use crate::embedded_icons::embedded_icon_png;

pub fn catalog_icon_bytes_for(app: &CatalogApp) -> Option<Vec<u8>> {
    if let Some(slice) = embedded_icon_png(&app.id) {
        return Some(slice.to_vec());
    }
    appimage_control_core::resolve_icon_image_url(app)
        .and_then(|u| appimage_control_core::fetch_url_bytes(&u))
}

pub fn batch_catalog_icons(catalog: &Catalog) -> Vec<Option<Vec<u8>>> {
    catalog.apps.iter().map(catalog_icon_bytes_for).collect()
}
