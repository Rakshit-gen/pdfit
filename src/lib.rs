//! pdfit: make a PDF fit an upload limit by recompressing the images in it
//! and nothing else.
pub mod doc;
pub mod matrix;
pub mod predictor;
pub mod raster;
pub mod recompress;
pub mod scan;

#[cfg(test)]
mod testpdf;
