use super::*;

#[test]
fn f3d_top_edge_stores_match_retained_witness() {
    let mut state = VertexState {
        mvp: [512, 0, 0, 0, 0, 512, 0, 0, 0, 0, 65536, 0, 0, 0, 0, 65536],
        vscale: [640, 480, 511, 0],
        vtrans: [640, 480, 511, 0],
        persp_norm: 65535,
        clip_ratio: 1,
        texture_scale: [0; 2],
    };
    for quarter in 0..4 {
        state.vtrans[1] = 480 + quarter;
        let got = [[-120, 128, 0], [120, 128, 0], [-120, 64, 0]]
            .map(|p| geometry_vertex(true, &state, p).screen);
        assert_eq!(got, [[39, quarter], [1239, quarter], [39, 240 + quarter]]);
    }
    state.mvp[0] = 409;
    state.mvp[5] = 546;
    state.mvp[12] = -65536;
    state.mvp[13] = -65536;
    state.vtrans[1] = 480;
    let got = [[16, 240, 0], [304, 240, 0], [16, 180, 0]]
        .map(|p| geometry_vertex(true, &state, p).screen);
    assert_eq!(got, [[63, 0], [1214, 0], [63, 240]]);
}

#[test]
fn f3dzex_clipped_screen_positions_match_the_command_path() {
    let state = VertexState {
        mvp: [
            0x4000, 0, 0, 0, 0, 0x4000, 0, 0, 0, 0, 0, 0x4000, 0, 0, 0, 0,
        ],
        vscale: [512, 512, 511, 0],
        vtrans: [640, 480, 511, 0],
        persp_norm: 65535,
        clip_ratio: 1,
        texture_scale: [65535; 2],
    };
    let mut planes = CLIP_PLANES;
    for plane in &mut planes[..4] {
        plane[3] /= 2;
    }
    let vertices = [[128, -80, 256], [384, -432, 768], [1440, -336, 768]]
        .map(|p| vertex(&state, p, [0; 2], [255; 4]));
    let expected: Vec<_> = clipped_triangles(&state, planes, vertices, 0, None)
        .into_iter()
        .map(|(v, _)| v.map(|v| v.screen))
        .collect();
    let output = geometry(false, &state, planes, vertices, 0);
    assert!(output.clipped);
    assert_eq!(output.generated, 2);
    assert_eq!(
        output
            .triangles
            .into_iter()
            .map(|v| v.map(|v| v.screen))
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn f3d_each_plane_generates_inside_stores_and_keeps_originals() {
    let state = VertexState {
        mvp: [512, 0, 0, 0, 0, 512, 0, 0, 0, 0, 512, 0, 0, 0, 0, 65536],
        vscale: [640, 480, 511, 0],
        vtrans: [640, 480, 511, 0],
        persp_norm: 65535,
        clip_ratio: 1,
        texture_scale: [0; 2],
    };
    let mut planes = CLIP_PLANES;
    for plane in &mut planes[..4] {
        plane[3] /= 2;
    }
    planes[5] = [0, 0, 1, 1];
    for axis in 0..3 {
        for sign in [-1, 1] {
            let mut points = [[-31, 19, 5], [27, -47, 9], [41, 53, -11]];
            points[2][axis] = sign * 223;
            let vertices = points.map(|p| geometry_vertex(true, &state, p));
            let mut draw_state = state;
            draw_state.vtrans[0] += 37;
            let result = geometry(true, &draw_state, planes, vertices, 0);
            assert!(result.clipped);
            assert_eq!(result.generated, 2, "axis={axis}, sign={sign}");
            assert_eq!(result.triangles.len(), 2, "axis={axis}, sign={sign}");
            for original in &vertices[..2] {
                assert!(result
                    .triangles
                    .iter()
                    .flatten()
                    .any(|v| v.position == original.position && v.screen == original.screen));
            }
            for v in result.triangles.iter().flatten() {
                assert!(
                    v.position[axis].abs() <= v.position[3] + 4,
                    "axis={axis}, sign={sign}: {v:?}"
                );
            }
        }
    }
}
