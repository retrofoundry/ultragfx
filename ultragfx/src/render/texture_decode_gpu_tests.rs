use super::{readback_requests, test_support};

#[test]
fn texture_decode_gpu_console_zero_rgba4_descriptor() {
    use crate::hle::texture_request::{prepare, TextureSource};
    use crate::profiling::{Recorder, Request};

    // second-tile-followup: producer word a5c1 gives ad6b RGB and passes alpha 170, not 171.
    let mut memory = vec![0; 0x110000];
    memory[0x10c000..0x10c002].copy_from_slice(&[0xa5, 0xc1]);
    let words: [(u32, u32); 9] = [
        (0xfd10001f, 0x0010c000),
        (0xf5101000, 0x07000000),
        (0xe6000000, 0),
        (0xf4000000, 0x0707c07c),
        (0xe7000000, 0),
        (0xe8000000, 0),
        (0xf5000000, 0x01000000),
        (0xf2000000, 0x01000000),
        (0xdf000000, 0),
    ];
    for (i, (hi, lo)) in words.into_iter().enumerate() {
        memory[0x1000 + i * 8..0x1004 + i * 8].copy_from_slice(&hi.to_be_bytes());
        memory[0x1004 + i * 8..0x1008 + i * 8].copy_from_slice(&lo.to_be_bytes());
    }
    let result = crate::hle::interpret(
        crate::RdramImage::new(&memory),
        0x1000,
        crate::hle::gbi::GbiUcode::F3dex2,
        crate::DataFormat::Fixed,
        None,
    );
    assert!(result.diags.is_empty(), "{:?}", result.diags);
    let tile = &result.rdp.tiles[1];
    let binding = prepare(&result.rdp, tile, 0, &Recorder::default()).unwrap();
    let TextureSource::Encoded(encoded) = &binding.source else {
        panic!("expected TMEM request");
    };
    let request = Request::capture(&result.rdp, tile, 0, "console-zero-rgba4", Ok(encoded));
    assert_eq!(result.rdp.tmem_bank.sample_tile(tile, 0).unwrap(), [170; 4]);
    let (device, queue) = crate::render::guarded_headless_device_forced_fallback();
    let decoded = pollster::block_on(readback_requests(&device, &queue, &[request])).unwrap();
    assert_eq!(decoded, vec![vec![170; 4]]);
}

#[test]
fn texture_decode_gpu_component_and_layout_readbacks() {
    let (device, queue) = crate::render::guarded_headless_device_forced_fallback();
    let vectors = test_support::vectors();
    let requests: Vec<_> = vectors
        .iter()
        .map(|vector| vector.request.clone())
        .collect();
    let decoded = pollster::block_on(readback_requests(&device, &queue, &requests)).unwrap();
    test_support::verify(&device, &vectors, &decoded);
}

#[test]
fn texture_decode_gpu_literal_load_readbacks() {
    use crate::hle::texture_request::{prepare, TextureSource};
    use crate::profiling::{Recorder, Request};

    let vectors: Vec<_> = crate::hle::tmem::load_tests::literal_load_vectors()
        .into_iter()
        .map(|vector| {
            let binding = prepare(&vector.rdp, &vector.tile, 0, &Recorder::default()).unwrap();
            let TextureSource::Encoded(encoded) = &binding.source else {
                panic!("{}: expected TMEM request", vector.name);
            };
            super::vectors::Vector {
                request: Request::capture(&vector.rdp, &vector.tile, 0, &vector.name, Ok(encoded)),
                name: vector.name,
                expected: vector.expected,
            }
        })
        .collect();
    let (device, queue) = crate::render::guarded_headless_device_forced_fallback();
    let requests: Vec<_> = vectors
        .iter()
        .map(|vector| vector.request.clone())
        .collect();
    let decoded = pollster::block_on(readback_requests(&device, &queue, &requests)).unwrap();
    test_support::assert_decoded(&vectors, &decoded);
}

#[test]
fn texture_decode_gpu_angrylion_fetch_readbacks() {
    let vectors: Vec<_> = crate::hle::angrylion_tests::decode_vectors()
        .into_iter()
        .map(|(name, request, expected)| super::vectors::Vector {
            name,
            request,
            expected,
        })
        .collect();
    let (device, queue) = crate::render::guarded_headless_device_forced_fallback();
    let requests: Vec<_> = vectors.iter().map(|v| v.request.clone()).collect();
    let decoded = pollster::block_on(readback_requests(&device, &queue, &requests)).unwrap();
    test_support::assert_decoded(&vectors, &decoded);
}
