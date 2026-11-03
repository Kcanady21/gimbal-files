// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded image decoding shared by the preview and the grid thumbnailer.
//!
//! The loader is told the target size in `size-prepared`, so a huge JPEG is
//! never allocated at full resolution. The decoded pixel buffer and source
//! dimensions are returned to the main thread, which alone creates textures.

use std::cell::Cell;
use std::io::Read;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gdk_pixbuf;
use gtk::prelude::*;

use fm_core::preview::{self, RenderLimit, RenderSize};

/// The largest decoded RGBA buffer one render may allocate (16 MP × 4).
pub const MAX_TEXTURE_BYTES: u64 = preview::MAX_RENDER_PIXELS * 4;

#[derive(Clone)]
pub(crate) struct DecodedImage {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub bytes: Vec<u8>,
    /// The source image after EXIF orientation, before scaling.
    pub source_width: i32,
    pub source_height: i32,
}

#[derive(Clone, Debug)]
pub(crate) enum ImageProblem {
    FileTooLarge,
    TooLarge { width: i32, height: i32 },
    Invalid(String),
    Limit(String),
}

pub(crate) fn read_bounded(
    path: &Path,
    limit: u64,
    allow_symlink: bool,
) -> Result<Vec<u8>, ImageProblem> {
    let metadata = if allow_symlink {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    }
    .map_err(|error| ImageProblem::Invalid(error.to_string()))?;
    if (!allow_symlink && metadata.file_type().is_symlink()) || !metadata.is_file() {
        return Err(ImageProblem::Invalid(
            "the thumbnail is not a regular file".to_string(),
        ));
    }
    if metadata.len() > limit {
        return Err(ImageProblem::FileTooLarge);
    }
    let mut file =
        std::fs::File::open(path).map_err(|error| ImageProblem::Invalid(error.to_string()))?;
    let mut bytes = Vec::with_capacity(64 * 1024);
    std::io::Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ImageProblem::Invalid(error.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(ImageProblem::FileTooLarge);
    }
    Ok(bytes)
}

/// Decode a local image at a bounded device-pixel target. `allow_symlink` is
/// true for the selected image (the preview may follow it) and false for a GIO
/// thumbnail cache file, which must be a regular file itself.
pub(crate) fn decode_image_file(
    path: &Path,
    target: RenderSize,
    allow_symlink: bool,
) -> Result<DecodedImage, ImageProblem> {
    #[cfg(test)]
    test_gate::wait(path);
    let bytes = read_bounded(path, preview::MAX_IMAGE_BYTES, allow_symlink)?;
    decode_image_bytes(&bytes, target)
}

pub(crate) fn decode_image_bytes(
    bytes: &[u8],
    target: RenderSize,
) -> Result<DecodedImage, ImageProblem> {
    decode_image_bytes_limited(bytes, target, true)
}

/// `bounded` false is used only for a generated PDF/thumbnail that was already
/// checked by its renderer. It still enforces the texture-byte bound below.
pub(crate) fn decode_image_bytes_limited(
    bytes: &[u8],
    target: RenderSize,
    bounded: bool,
) -> Result<DecodedImage, ImageProblem> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    let dimensions: Rc<Cell<Option<(i32, i32)>>> = Rc::new(Cell::new(None));
    let oversized = Rc::new(Cell::new(false));
    {
        let weak = loader.downgrade();
        let dimensions = dimensions.clone();
        let oversized = oversized.clone();
        loader.connect_size_prepared(move |_, width, height| {
            dimensions.set(Some((width, height)));
            let Some(loader) = weak.upgrade() else {
                return;
            };
            if bounded && preview::image_target_size(width, height).is_none() {
                oversized.set(true);
                loader.set_size(1, 1);
                return;
            }
            let (target_width, target_height) =
                preview::fit_within(width, height, target.width, target.height);
            loader.set_size(target_width.max(1), target_height.max(1));
        });
    }
    if let Err(error) = loader.write(bytes) {
        let _ = loader.close();
        return Err(ImageProblem::Invalid(error.to_string()));
    }
    if let Err(error) = loader.close() {
        return Err(ImageProblem::Invalid(error.to_string()));
    }
    if oversized.get() {
        let (width, height) = dimensions.get().unwrap_or((0, 0));
        return Err(ImageProblem::TooLarge { width, height });
    }
    let Some(pixbuf) = loader.pixbuf() else {
        return Err(ImageProblem::Invalid(
            "the image decoder returned no pixels".to_string(),
        ));
    };
    let (decoded_width, decoded_height) = (pixbuf.width(), pixbuf.height());
    // EXIF orientation: the pixbuf loader exposes the decoded pixels but does
    // not rotate them, so do it before reading the buffer. Orientation can
    // swap width and height.
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let pixbuf = if pixbuf.n_channels() == 3 {
        pixbuf
            .add_alpha(false, 0, 0, 0)
            .map_err(|error| ImageProblem::Invalid(error.to_string()))?
    } else if pixbuf.n_channels() == 4 {
        pixbuf
    } else {
        return Err(ImageProblem::Invalid(format!(
            "unsupported channel count {}",
            pixbuf.n_channels()
        )));
    };
    let stride = pixbuf.rowstride() as usize;
    let bytes = pixbuf.read_pixel_bytes().as_ref().to_vec();
    if stride as u64 * u64::from(pixbuf.height() as u32) > MAX_TEXTURE_BYTES {
        return Err(ImageProblem::Limit(RenderLimit::Pixels.message()));
    }
    let (source_width, source_height) = dimensions.get().unwrap_or((decoded_width, decoded_height));
    let oriented = if decoded_width != pixbuf.width() || decoded_height != pixbuf.height() {
        (source_height, source_width)
    } else {
        (source_width, source_height)
    };
    Ok(DecodedImage {
        width: pixbuf.width(),
        height: pixbuf.height(),
        stride,
        bytes,
        source_width: oriented.0,
        source_height: oriented.1,
    })
}

/// Build a main-thread texture from decoded bytes.
pub(crate) fn texture(image: &DecodedImage) -> gtk::gdk::MemoryTexture {
    let bytes = gtk::glib::Bytes::from_owned(image.bytes.clone());
    gtk::gdk::MemoryTexture::new(
        image.width,
        image.height,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        image.stride,
    )
}

#[cfg(test)]
mod test_gate {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    pub(crate) struct TestGate {
        prefix: PathBuf,
        released: Mutex<bool>,
        ready: Condvar,
        entered: AtomicUsize,
    }

    impl TestGate {
        pub(crate) fn new(prefix: &Path) -> Self {
            Self {
                prefix: prefix.to_path_buf(),
                released: Mutex::new(false),
                ready: Condvar::new(),
                entered: AtomicUsize::new(0),
            }
        }

        pub(crate) fn release(&self) {
            if let Ok(mut released) = self.released.lock() {
                *released = true;
                self.ready.notify_all();
            }
        }

        pub(crate) fn entered(&self) -> usize {
            self.entered.load(Ordering::SeqCst)
        }

        pub(crate) fn wait(&self, path: &Path) {
            if !path.starts_with(&self.prefix) {
                return;
            }
            self.entered.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut released) = self.released.lock() {
                while !*released {
                    match self.ready.wait(released) {
                        Ok(next) => released = next,
                        Err(_) => break,
                    }
                }
            }
        }
    }

    pub(crate) struct GateGuard {
        gate: Arc<TestGate>,
    }

    impl GateGuard {
        pub(crate) fn release(&self) {
            self.gate.release();
        }

        pub(crate) fn entered(&self) -> usize {
            self.gate.entered()
        }
    }

    impl Drop for GateGuard {
        fn drop(&mut self) {
            self.gate.release();
            GATES
                .lock()
                .unwrap()
                .retain(|gate| !Arc::ptr_eq(gate, &self.gate));
        }
    }

    static GATES: Mutex<Vec<Arc<TestGate>>> = Mutex::new(Vec::new());

    /// A controlled barrier for scheduler tests. Only paths under the test's
    /// own prefix wait. Several tests can hold different gates at once; the
    /// returned guard releases and removes its own gate on drop.
    pub(crate) fn set(prefix: &Path) -> GateGuard {
        let gate = Arc::new(TestGate::new(prefix));
        GATES.lock().unwrap().push(gate.clone());
        GateGuard { gate }
    }

    pub(crate) fn wait(path: &Path) {
        let gates = GATES.lock().unwrap().clone();
        for gate in gates {
            gate.wait(path);
        }
    }
}

#[cfg(test)]
pub(crate) use test_gate::{TestGate, set as set_decode_gate};

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from("/tmp").join(format!(
                "gimbal-preview-image-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn a_generated_high_detail_image_decodes_at_the_requested_device_pixels() {
        let scratch = Scratch::new();
        let path = scratch.path.join("high detail.png");
        let pixbuf = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 1600, 1200)
            .expect("the fixture pixbuf is created");
        pixbuf
            .savev(&path, "png", &[])
            .expect("the fixture PNG is written");
        let before = std::fs::read(&path).unwrap();
        let image = decode_image_file(
            &path,
            RenderSize {
                width: 800,
                height: 800,
            },
            true,
        )
        .expect("the generated image decodes");
        assert_eq!((image.source_width, image.source_height), (1600, 1200));
        assert!(image.width <= 800 && image.height <= 800);
        assert_eq!((image.width, image.height), (800, 600));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "source bytes unchanged"
        );
    }
}
