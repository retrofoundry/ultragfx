use super::interp::{interpret, interpret_with_bindings};
use crate::inspect::{WalkStep, WalkTermination};
use crate::tests::memory_failures::StrictMemory;
use crate::{DataFormat, DiagKind, Microcode, RdramImage};
use n64_gbi::encode::*;
use std::ops::ControlFlow;

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_be_bytes()).collect()
}

fn commands(commands: &[(u32, u32)]) -> Vec<u8> {
    commands.iter().flat_map(|&(a, b)| words(&[a, b])).collect()
}

#[derive(Debug, PartialEq)]
struct ObjectRectangle {
    tile: u8,
    descriptor: super::rdp::TileDescriptor,
    bounds: crate::scene::TexRectBounds,
    texture: [i16; 4],
}

fn object_rectangles(
    bytes: &[u8],
    entry: u64,
) -> (super::interp::InterpResult, Vec<ObjectRectangle>) {
    let mut rectangles = Vec::new();
    let result = interpret(
        RdramImage::new(bytes),
        entry,
        Microcode::S2dex2.into(),
        DataFormat::Fixed,
        Some(&mut |step: WalkStep<'_>| {
            for emission in step.emissions {
                if let crate::inspect::Emission::TexRect {
                    rect,
                    tile,
                    uls,
                    ult,
                    dsdx,
                    dtdy,
                    ..
                } = emission
                {
                    rectangles.push(ObjectRectangle {
                        tile: *tile,
                        descriptor: step.state.tiles[usize::from(*tile)].clone(),
                        bounds: *rect,
                        texture: [*uls, *ult, *dsdx, *dtdy],
                    });
                }
            }
            ControlFlow::Continue(())
        }),
    );
    (result, rectangles)
}

fn rdp_display_list(stream: &[u8]) -> Vec<u8> {
    let mut list = Vec::new();
    let mut packets = stream.as_chunks::<8>().0.iter();
    while let Some(packet) = packets.next() {
        let mut packet = *packet;
        packet[0] |= 0xc0;
        list.extend(packet);
        if matches!(packet[0], 0xe4 | 0xe5) {
            let coordinates = packets.next().unwrap();
            list.extend([0xe1, 0, 0, 0]);
            list.extend(&coordinates[..4]);
            list.extend([0xf1, 0, 0, 0]);
            list.extend(&coordinates[4..]);
        }
    }
    list.extend(commands(&[gsp_enddl()]));
    list
}

#[test]
fn object_rectangles_match_console_rdp_state() {
    fn variant(list: &[u8], cycle: Option<u8>, flags: u8) -> Vec<u8> {
        let mut output = Vec::new();
        if cycle == Some(1) {
            output.extend(commands(&[
                (0xf5101000, 0x01080200),
                (0xf2000000, 0x0107c07c),
                (0xf5101000, 0x03080200),
                (0xf2000000, 0x0307c07c),
            ]));
        }
        for packet in list.as_chunks::<8>().0 {
            let mut packet = *packet;
            if packet[0] == 0x0b {
                packet[7] |= flags;
            }
            if packet[0] == 0xef {
                if let Some(cycle) = cycle {
                    packet[1] = (packet[1] & !0x30) | (cycle << 4);
                }
            }
            output.extend(packet);
            if packet[0] == 0xdf {
                break;
            }
        }
        output
    }

    macro_rules! fixture {
        ($name:literal, $base:expr) => {
            (
                $name,
                $base,
                include_bytes!(concat!("s2dex_witnesses/", $name, ".input.bin")) as &[u8],
                include_bytes!(concat!("s2dex_witnesses/", $name, ".rdp.bin")) as &[u8],
            )
        };
    }
    for (name, base, page, stream) in [
        fixture!("c5-s1", 0xc40c0),
        fixture!("c5-objx-161", 0xc40c0),
        fixture!("c5-objx-162", 0xc40c0),
        fixture!("c5-objx-163", 0xc40c0),
        fixture!("c5-scale-0300", 0xc40c0),
        fixture!("c5-scale-0555", 0xc40c0),
        fixture!("c5-imagew-1000", 0xc40c0),
        fixture!("c5-both-flips", 0xc40c0),
        fixture!("c5-bilerp", 0xc40c0),
        fixture!("c5-shrink1", 0xc40c0),
        fixture!("c5-shrink2", 0xc40c0),
        fixture!("c5-widen", 0xc40c0),
        fixture!("c5-bilerp-shrink1", 0xc40c0),
        fixture!("c5-bilerp-scale-0555", 0xc40c0),
        fixture!("c5-ci4", 0xc40c0),
        fixture!("c6-asymmetric-mode-00", 0xbd940),
        fixture!("c6-asymmetric-mode-08", 0xbd940),
        fixture!("c5-add-mode-28-1cycle", 0xa9940),
        fixture!("c5-add-mode-30-1cycle", 0xa9940),
        fixture!("c5-add-mode-38-1cycle", 0xa9940),
        fixture!("c5-add-mode-48-1cycle", 0xa9940),
        fixture!("c5-add-mode-50-1cycle", 0xa9940),
        fixture!("c5-add-mode-58-1cycle", 0xa9940),
        fixture!("c5-add-mode-60-1cycle", 0xa9940),
        fixture!("c5-add-mode-68-1cycle", 0xa9940),
        fixture!("c5-add-mode-70-1cycle", 0xa9940),
        fixture!("c5-add-mode-78-1cycle", 0xa9940),
        fixture!("c5-add-mode-08-2cycle", 0xa9940),
        fixture!("c5-add-mode-18-2cycle", 0xa9940),
        fixture!("c5-copy", 0x7e400),
        fixture!("c8-four-same", 0x7e400),
        fixture!("c8-load-draw-load", 0x7e400),
        fixture!("c8-rect-combined", 0x8ee80),
        fixture!("c8-rectR-combined", 0x8ee80),
        fixture!("c8-repeat-moved", 0x8ee80),
        fixture!("rect-recip-0555-edge", 0xda1c0),
        fixture!("rect-r-recip-identity-negative", 0xc2bc0),
        fixture!("rect-recip-0802-lsb", 0xd6c80),
        fixture!("rect-recip-0804-lsb", 0xd6c80),
        fixture!("rect-recip-1004-lsb", 0xd6c80),
        fixture!("rect-r-recip-0802-position", 0xd6c80),
    ] {
        let entry = base + 0x1000;
        let mut bytes = vec![0; base + page.len() + stream.len() * 2 + 32];
        bytes[base..base + page.len()].copy_from_slice(page);
        let reference = rdp_display_list(stream);
        let copy = page[0x1000..]
            .as_chunks::<8>()
            .0
            .iter()
            .any(|packet| packet[0] == 0xef && packet[1] & 0x30 == 0x20);
        for cycle in [None, Some(0), Some(1)] {
            if copy && cycle.is_some() {
                continue;
            }
            for flags in 0..8 {
                let input = variant(&page[0x1000..], cycle, flags);
                bytes[entry..entry + input.len()].copy_from_slice(&input);
                let (actual, rectangles) = object_rectangles(&bytes, entry as u64);
                let expected_input = variant(&reference, cycle, 0);
                bytes[entry..entry + expected_input.len()].copy_from_slice(&expected_input);
                let (expected, reference_rectangles) = object_rectangles(&bytes, entry as u64);
                let label = format!("{name}, cycle {cycle:?}, flags {flags}");
                assert!(expected.diags.is_empty(), "{label}: {:?}", expected.diags);
                assert_eq!(expected.termination, WalkTermination::End, "{label}");
                assert!(actual.diags.is_empty(), "{label}: {:?}", actual.diags);
                assert_eq!(actual.termination, WalkTermination::End, "{label}");
                assert_eq!(rectangles, reference_rectangles, "{label}");
                assert_eq!(actual.rdp, expected.rdp, "{label}");
            }
        }
    }
}

#[test]
fn committed_object_task_prevents_stale_depth_alias() {
    use crate::tests::dl_builder::DlBuilder;
    use n64_gbi::consts::*;

    for reject_objects in [false, true] {
        let mut b = DlBuilder::new();
        let source = b.bytes(8, &[0x5b; 8]);
        let object = b.bytes(8, &words(&[0x1033, source, 0, 0, 1, u32::MAX]));
        let vertices = b.vertices(&[[-1, -1], [1, -1], [0, 1]].map(|[x, y]| VtxColored {
            x,
            y,
            z: 0,
            flag: 0,
            s: 0,
            t: 0,
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        }));
        let clear = b.list(
            "clear",
            &[
                gdp_set_depth_image(0x20000),
                gdp_set_color_image(0, 2, 4, 0x20000),
                gdp_set_scissor(0, 0, 0, 16, 16),
                gdp_set_cycle_type(3),
                gdp_set_fill_color(0xfffcfffc),
                gdp_fill_rectangle(0, 0, 12, 12),
                gsp_enddl(),
            ],
        );
        let objects = b.list(
            "objects",
            &[
                gdp_set_color_image(0, 2, 4, 0x10000),
                gsp_obj_load_txtr(object),
                (
                    if reject_objects {
                        0xd9c0f9fa
                    } else {
                        0xe0000000
                    },
                    0,
                ),
                gsp_enddl(),
            ],
        );
        let color = CcPass {
            a: 15,
            b: 15,
            c: 31,
            d: 4,
        };
        let alpha = CcPass {
            a: 7,
            b: 7,
            c: 7,
            d: 4,
        };
        let draw = b.list(
            "draw",
            &[
                gdp_set_cycle_type(0),
                gdp_set_combine_lerp(color, alpha, color, alpha),
                gdp_set_render_mode(G_RM_OPA_SURF | Z_CMP | Z_UPD, G_RM_OPA_SURF2),
                gsp_set_geometrymode(G_ZBUFFER | G_SHADE),
                gsp_vertex(0, 3, vertices),
                gsp_1triangle(0, 1, 2),
                gsp_enddl(),
            ],
        );
        let built = b.finish("clear");
        let mut cpu = crate::profiling::CpuInterpreter::default();
        cpu.begin_frame(4, 4);
        for (entry, microcode) in [(clear, Microcode::F3dex2), (objects, Microcode::S2dex2)] {
            let (summary, diags) = cpu.process(
                RdramImage::new(&built.rdram),
                entry.into(),
                microcode,
                DataFormat::Fixed,
            );
            if entry == objects && reject_objects {
                assert_eq!(summary.termination, WalkTermination::Rejected);
            } else {
                assert_eq!(summary.termination, WalkTermination::End);
                assert!(diags.is_empty(), "{diags:?}");
            }
        }
        let (summary, diags) = cpu.process(
            RdramImage::new(&built.rdram),
            draw.into(),
            Microcode::F3dex2,
            DataFormat::Fixed,
        );
        if reject_objects {
            assert!(diags
                .iter()
                .any(|d| d.kind == DiagKind::UnsupportedDepthAlias { address: 0x20000 }));
            assert_eq!(summary.tris, 0);
        } else {
            assert!(diags.is_empty(), "{diags:?}");
            assert_eq!(summary.tris, 1);
        }
    }
}

#[test]
fn texture_loads_match_safe_205_console_streams() {
    let cases = [
        (
            0x00001033,
            0x00ff,
            0x01000000,
            0x4da80,
            include_bytes!("s2dex_witnesses/c4-block.rdp.bin"),
        ),
        (
            0x00fc1034,
            0x001f,
            0x007f0000,
            0x4da80,
            include_bytes!("s2dex_witnesses/c4-tile.rdp.bin"),
        ),
        (
            0x00000030,
            0x0100000f,
            0,
            0x4d880,
            include_bytes!("s2dex_witnesses/c4-tlut.rdp.bin"),
        ),
        (
            0x00fc1834,
            0x001f,
            0x007f0000,
            0x4da80,
            include_bytes!("s2dex_witnesses/c4-odd-type.rdp.bin"),
        ),
    ];
    for (ty, size, line, source, stream) in cases {
        let mut bytes = commands(&[
            (0xe7000000, 0),
            (0xff10013f, 0x150900),
            (0xed000000, 0x005003c0),
            (0xef000cf0, 0x0f0a4000),
            (0xfcffffff, 0xfffcf279),
            gsp_obj_load_txtr(0x200),
            gsp_enddl(),
        ]);
        bytes.resize(0x200, 0);
        bytes.extend(words(&[ty, source, size, line, source, u32::MAX]));
        bytes.resize(0x4d880, 0);
        bytes.extend((0..8192).map(|i| (i * 37 + i / 17) as u8));
        let actual = interpret(
            RdramImage::new(&bytes),
            0,
            Microcode::S2dex2.into(),
            DataFormat::Fixed,
            None,
        );
        assert!(actual.diags.is_empty(), "{ty:x}: {:?}", actual.diags);
        assert_eq!(actual.termination, WalkTermination::End);

        let mut reference = stream.to_vec();
        for word in reference.as_chunks_mut::<8>().0 {
            word[0] |= 0xc0; // RDP opcodes use only the low six bits.
        }
        reference.extend(commands(&[gsp_enddl()]));
        bytes[..reference.len()].copy_from_slice(&reference);
        let expected = interpret(
            RdramImage::new(&bytes),
            0,
            Microcode::S2dex2.into(),
            DataFormat::Fixed,
            None,
        );
        assert!(expected.diags.is_empty(), "{:?}", expected.diags);
        assert_eq!(actual.rdp, expected.rdp, "{ty:x}");
        assert!(actual
            .rdp
            .tmem_bank
            .profile_bank()
            .bytes
            .iter()
            .any(|&b| b != 0));
    }
}

#[test]
fn status_masks_control_loads_and_preserve_other_bits() {
    for sid in [0, 4, 8, 12] {
        for (initial, flag, mask, addresses, status) in [
            (0, 0, 0, [0, 0], 0),
            (0, 1, 0, [0x400, 0x800], 0),
            (0, 1, u32::MAX, [0x400, 0x400], 1),
            (0xa5a51234, 0x56, 0xff, [0x400, 0x400], 0xa5a51256),
            (0xa5a51256, 0x56, 0xff, [0, 0], 0xa5a51256),
        ] {
            let mut bytes = commands(&[
                gsp_set_status(sid, initial),
                gsp_segment(1, 0x200),
                gsp_segment(2, 0x400),
                gsp_obj_load_txtr(0x01000000),
                gsp_obj_load_txtr(0x01000018),
                gsp_enddl(),
            ]);
            bytes.resize(0x200, 0);
            for source in [0x02000000, 0x02000400] {
                bytes.extend(words(&[0x1033, source, 1, sid, flag, mask]));
            }
            bytes.resize(0x810, 0x9b);
            let mut seen = Vec::new();
            let result = interpret(
                StrictMemory::new(&bytes),
                0,
                Microcode::S2dex2.into(),
                DataFormat::Fixed,
                Some(&mut |step: WalkStep<'_>| {
                    if step.words[0].w0 >> 24 == 5 {
                        seen.push((step.state.texture_image.addr, step.state.obj_status));
                    }
                    ControlFlow::Continue(())
                }),
            );
            assert!(result.diags.is_empty(), "{:?}", result.diags);
            assert_eq!(result.termination, WalkTermination::End);
            let mut statuses = [0; 4];
            statuses[sid as usize / 4] = status;
            assert_eq!(
                seen,
                addresses.map(|addr| (addr, statuses)),
                "sid {sid}, mask {mask:x}"
            );
        }
    }
}

#[test]
fn matrix_partial_loads_and_reload_match_safe_205() {
    let mut bytes = commands(&[
        (0xe0000000, 0),
        gsp_obj_matrix(0x200),
        gsp_obj_sub_matrix(0x218),
        (0xdc0f0000, 0x220),
        gsp_obj_render_mode(0x7f),
        gsp_set_status(0, 0x11111111),
        gsp_set_status(4, 0x22222222),
        gsp_set_status(8, 0x33333333),
        gsp_set_status(12, 0x44444444),
        (0xe1000000, 0x8000),
        (0xdd0007ff, 0x1000),
        gsp_enddl(),
    ]);
    bytes.resize(0x200, 0);
    bytes.extend(words(&[
        0x11110000, 0x22220000, 0x33330000, 0x44440000, 0x01230456, 0x04000200,
    ]));
    bytes.extend(words(&[0x07890abc, 0x03000500]));
    bytes.extend(words(&[0x12345678, 0xedcba988, 0x76543210, 0x89abcdf0]));
    let mut states = Vec::new();
    let result = interpret_with_bindings(
        StrictMemory::new(&bytes),
        0,
        Microcode::S2dex2.into(),
        DataFormat::Fixed,
        Default::default(),
        Default::default(),
        Some(&mut |step: WalkStep<'_>| {
            states.push((
                step.state.obj_mtx,
                step.state.obj_render_mode,
                step.state.obj_status,
            ));
            ControlFlow::Continue(())
        }),
        Default::default(),
        &[(0x1000, Microcode::S2dex2)],
    );
    assert!(result.diags.is_empty(), "{:?}", result.diags);
    assert_eq!(result.termination, WalkTermination::End);
    let identity = crate::ObjMtx {
        a: 0x10000,
        d: 0x10000,
        base_scale_x: 0x400,
        base_scale_y: 0x400,
        ..Default::default()
    };
    let full = crate::ObjMtx {
        a: 0x11110000,
        b: 0x22220000,
        c: 0x33330000,
        d: 0x44440000,
        x: 0x123,
        y: 0x456,
        base_scale_x: 0x400,
        base_scale_y: 0x200,
    };
    let sub = crate::ObjMtx {
        x: 0x789,
        y: 0xabc,
        base_scale_x: 0x300,
        base_scale_y: 0x500,
        ..full
    };
    let linear = crate::ObjMtx {
        a: 0x12345678,
        b: 0xedcba988u32 as i32,
        c: 0x76543210,
        d: 0x89abcdf0u32 as i32,
        ..sub
    };
    assert_eq!(states[0], (identity, 0, [0; 4]));
    assert_eq!(states[1].0, full);
    assert_eq!(states[2].0, sub);
    assert_eq!(states[3].0, linear);
    assert_eq!(
        states[8],
        (
            linear,
            0x7f,
            [0x11111111, 0x22222222, 0x33333333, 0x44444444]
        )
    );
    assert_eq!(states[10], (identity, 0, [0; 4]));
    assert_eq!(result.rdp, Default::default());
}

#[test]
fn object_read_failures_report_the_missing_field() {
    use crate::{Diagnostic, MemoryAccess as A, MemoryError, MemoryErrorKind};
    for (word, extent, field, length, access) in [
        (0x05000017, 24, 20, 4, A::ObjTxtr),
        (0xdc170000, 24, 22, 2, A::ObjMtx),
        (0xdc070002, 8, 6, 2, A::ObjMtx),
        (0xdc0f0000, 16, 12, 4, A::ObjMtx),
        (0x01000000, 24, 23, 1, A::ObjSprite),
        (0xda000000, 24, 23, 1, A::ObjSprite),
        (0x0700002f, 48, 47, 1, A::ObjSprite),
        (0x0800002f, 48, 47, 1, A::ObjSprite),
    ] {
        let mut bytes = commands(&[(word, 16), gsp_enddl()]);
        bytes.resize(16 + extent - 1, 0);
        let result = interpret(
            StrictMemory::new(&bytes),
            0,
            Microcode::S2dex2.into(),
            DataFormat::Fixed,
            None,
        );
        assert_eq!(result.termination, WalkTermination::Bounds);
        assert_eq!(result.commands, 1);
        assert_eq!(
            result.diags,
            [Diagnostic {
                at: 0,
                kind: DiagKind::MemoryRead {
                    access,
                    error: MemoryError {
                        address: 16 + field,
                        length,
                        kind: MemoryErrorKind::OutOfBounds
                    }
                }
            }]
        );
    }
}

#[test]
fn unmodeled_status_ids_and_dma_sizes_reject_the_task() {
    for sid in [2, 16, 0xffff] {
        for cmd in [gsp_set_status(sid, 1), gsp_obj_load_txtr(16)] {
            let mut bytes = commands(&[cmd, gsp_enddl()]);
            bytes.extend(words(&[
                0x1033,
                0x1000,
                0xff,
                0x01000000 | sid,
                1,
                u32::MAX,
            ]));
            let result = interpret(
                StrictMemory::new(&bytes),
                0,
                Microcode::S2dex2.into(),
                DataFormat::Fixed,
                None,
            );
            assert_eq!(result.termination, WalkTermination::Rejected);
            assert_eq!(result.commands, 1);
            assert_eq!(
                result.diags[0].kind,
                DiagKind::UnsupportedObjStatus { sid: sid as u16 }
            );
            assert!(result.diags[0].kind.to_string().contains("HLE limit"));
        }
    }
    for word in [
        0x05000016, 0x05000018, 0xdc070000, 0xdc170002, 0xdc080008, 0x07000017, 0x08000017,
        0x07000030,
    ] {
        let bytes = commands(&[(word, 0xfffffff0), gsp_enddl()]);
        let result = interpret(
            StrictMemory::new(&bytes),
            0,
            Microcode::S2dex2.into(),
            DataFormat::Fixed,
            None,
        );
        assert_eq!(result.termination, WalkTermination::Rejected);
        assert_eq!(result.commands, 1);
        assert_eq!(
            result.diags[0].kind,
            DiagKind::UnsupportedCommandParameters {
                opcode: (word >> 24) as u8
            }
        );
    }
}

#[cfg(all(not(target_arch = "wasm32"), target_pointer_width = "64"))]
#[test]
fn object_commands_reject_host_layout_without_dereferencing() {
    use super::host_mem::{HostMemory, HostRam};
    for (word, access) in [
        (0x05000017, crate::MemoryAccess::ObjTxtr),
        (0xdc170000, crate::MemoryAccess::ObjMtx),
        (0x01000000, crate::MemoryAccess::ObjSprite),
        (0xda000000, crate::MemoryAccess::ObjSprite),
        (0x0700002f, crate::MemoryAccess::ObjSprite),
        (0x0800002f, crate::MemoryAccess::ObjSprite),
    ] {
        let words = [word, 1u64, 0xdf000000, 0];
        let result = crate::inspect::walk(
            unsafe { HostMemory::new(HostRam::new(&[])) },
            words.as_ptr() as u64,
            Microcode::S2dex2,
            DataFormat::Float,
            &mut |_: WalkStep<'_>| ControlFlow::Continue(()),
        );
        assert_eq!(result.termination, WalkTermination::MemoryRead);
        assert!(
            matches!(result.diagnostics[0].kind, DiagKind::MemoryRead { access: a, error } if a == access && error.kind == crate::MemoryErrorKind::UnsupportedFormat)
        );
    }
}
