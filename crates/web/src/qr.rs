//! The World App connector link, drawn as a scannable code. Replaces
//! `WorldIdQr` in `web/src/app/c/[token]/ChallengeActions.tsx` and its use of
//! the `qrcode` npm package.
//!
//! Encoding is the `qrcode` crate (no dependencies of its own, used here
//! without its `image` and `svg` features): a synchronous, pure data
//! transform, so the code renders in the same pass as the URI that produced it
//! and has no loading state. The crate hands back the module matrix and the
//! drawing is ours, like the TypeScript's, in fixed black-on-white regardless
//! of theme: a QR code's contrast is a scanning requirement, not a themable
//! surface.

use std::fmt::Write as _;

use leptos::prelude::*;
use qrcode::{Color, EcLevel, QrCode};

/// Modules of quiet zone around the code, matching the library's own default
/// margin: enough for a phone camera to lock on without hunting.
const QUIET_ZONE_MODULES: usize = 4;

/// A square grid of modules, dark or light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    size: usize,
    dark: Vec<bool>,
}

impl QrMatrix {
    /// Encodes `text` at error-correction level M. Fails only when the text
    /// is too long for any QR version.
    pub fn encode(text: &str) -> Result<Self, qrcode::types::QrError> {
        let code = QrCode::with_error_correction_level(text, EcLevel::M)?;
        Ok(Self {
            size: code.width(),
            dark: code
                .to_colors()
                .into_iter()
                .map(|color| color == Color::Dark)
                .collect(),
        })
    }

    /// Modules per side, not counting the quiet zone.
    pub fn size(&self) -> usize {
        self.size
    }

    pub fn is_dark(&self, row: usize, column: usize) -> bool {
        row < self.size && column < self.size && self.dark[row * self.size + column]
    }

    /// Side of the drawing in module units, quiet zone included.
    pub fn dimension(&self) -> usize {
        self.size + QUIET_ZONE_MODULES * 2
    }

    /// One SVG path covering every dark module as a unit square, offset by the
    /// quiet zone. A single path keeps a version-10 code at one DOM node
    /// instead of a thousand.
    pub fn path_data(&self) -> String {
        let mut path = String::new();
        for row in 0..self.size {
            for column in 0..self.size {
                if self.is_dark(row, column) {
                    // Writing to a String cannot fail.
                    let _ = write!(
                        path,
                        "M{} {}h1v1h-1z",
                        column + QUIET_ZONE_MODULES,
                        row + QUIET_ZONE_MODULES
                    );
                }
            }
        }
        path
    }
}

#[component]
pub fn WorldIdQr(#[prop(into)] uri: String) -> impl IntoView {
    let Ok(matrix) = QrMatrix::encode(&uri) else {
        return view! {
            <p class="px-2 py-6 text-center text-xs text-faint">
                "This link is too long to draw as a code. Use the button above."
            </p>
        }
        .into_any();
    };
    let dimension = matrix.dimension();
    view! {
        <svg
            viewBox=format!("0 0 {dimension} {dimension}")
            class="mx-auto h-44 w-44"
            role="img"
            aria-label="World ID QR code — scan with the World App"
            shape-rendering="crispEdges"
        >
            <rect width=dimension height=dimension fill="#ffffff" />
            <path d=matrix.path_data() fill="#000000" />
        </svg>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://world.org/verify?t=mock";

    #[test]
    fn a_connector_link_encodes_to_a_square_matrix_with_finder_patterns() {
        let matrix = QrMatrix::encode(LINK).unwrap();
        assert!(matrix.size() >= 21, "version 1 is 21 modules a side");
        assert_eq!((matrix.size() - 17) % 4, 0, "sizes are 17 + 4 * version");
        // The three finder patterns start with a dark corner module.
        let last = matrix.size() - 1;
        assert!(matrix.is_dark(0, 0));
        assert!(matrix.is_dark(0, last));
        assert!(matrix.is_dark(last, 0));
    }

    #[test]
    fn the_drawing_leaves_a_quiet_zone_on_every_side() {
        let matrix = QrMatrix::encode(LINK).unwrap();
        assert_eq!(matrix.dimension(), matrix.size() + 8);
        assert!(
            matrix.path_data().starts_with("M4 4h1v1h-1z"),
            "the first dark module sits at the margin, not the edge"
        );
    }

    #[test]
    fn the_path_has_one_square_per_dark_module() {
        let matrix = QrMatrix::encode(LINK).unwrap();
        let dark = (0..matrix.size())
            .flat_map(|row| (0..matrix.size()).map(move |column| (row, column)))
            .filter(|&(row, column)| matrix.is_dark(row, column))
            .count();
        assert_eq!(matrix.path_data().matches('M').count(), dark);
    }

    #[test]
    fn the_same_link_always_draws_the_same_code() {
        assert_eq!(QrMatrix::encode(LINK), QrMatrix::encode(LINK));
        assert_ne!(
            QrMatrix::encode(LINK).unwrap(),
            QrMatrix::encode("https://world.org/verify?t=other").unwrap()
        );
    }

    #[test]
    fn a_link_too_long_for_any_version_is_an_error_not_a_panic() {
        assert!(QrMatrix::encode(&"x".repeat(4_000)).is_err());
    }

    #[test]
    fn coordinates_outside_the_grid_are_light() {
        let matrix = QrMatrix::encode(LINK).unwrap();
        assert!(!matrix.is_dark(matrix.size(), 0));
        assert!(!matrix.is_dark(0, matrix.size()));
    }
}
