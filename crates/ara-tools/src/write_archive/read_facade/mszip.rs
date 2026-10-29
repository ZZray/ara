//! CAB MSZIP dictionary adapter using the unchanged existing raw DEFLATE backend.
use super::*;
pub(super) fn inflate(packed: &[u8], dictionary: &[u8], maximum: usize) -> Result<Vec<u8>> {
    if dictionary.len() > 32768 || maximum > 32768 {
        return Err(error("Invalid CAB MSZIP dictionary or output bound"));
    }
    let mut framed = Vec::with_capacity(packed.len() + dictionary.len() + 5);
    if !dictionary.is_empty() {
        // RFC1951 stored block: BFINAL=0, BTYPE=00, then align to a byte,
        // LEN/NLEN and verbatim history. The following original bitstream
        // begins at exactly the same byte boundary as inflateRaw(dictionary).
        let length = dictionary.len() as u16;
        framed.push(0);
        framed.extend_from_slice(&length.to_le_bytes());
        framed.extend_from_slice(&(!length).to_le_bytes());
        framed.extend_from_slice(dictionary);
    }
    framed.extend_from_slice(packed);
    let mut decoder = flate2::Decompress::new(false);
    let mut output = vec![0u8; dictionary.len() + maximum + 1];
    let status = decoder
        .decompress(&framed, &mut output, flate2::FlushDecompress::Finish)
        .map_err(|err| error(err.to_string()))?;
    let produced = decoder.total_out() as usize;
    if produced > dictionary.len() + maximum {
        return Err(error(format!("Cannot create a Buffer larger than {maximum} bytes")));
    }
    if status != flate2::Status::StreamEnd || produced < dictionary.len() {
        return Err(error("unexpected end of file"));
    }
    Ok(output[dictionary.len()..produced].to_vec())
}
