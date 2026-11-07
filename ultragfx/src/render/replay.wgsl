struct ReplayOutput {
    @location(0) stamps: vec2<u32>,
    @location(1) code: u32,
    @builtin(frag_depth) depth: f32,
};
struct DeltaOutput {
    @location(0) code: u32,
    @builtin(frag_depth) depth: f32,
};

fn replay_alpha(in: VsOut) {
    if combiner.alpha_flags != 0u {
        alpha_discard(eval_combiner(in), in.clip_position.xy);
    }
}

@fragment
fn fs_replay(in: VsOut) -> ReplayOutput {
    let code = delta_code(incoming_delta(in));
    replay_alpha(in);
    return ReplayOutput(vec2<u32>(combiner.coverage_slot), code, fragment_depth(in.clip_position.z));
}

@fragment
fn fs_replay_decal(in: VsOut) -> ReplayOutput {
    let code = delta_code(incoming_delta(in));
    decal_test(in);
    replay_alpha(in);
    return ReplayOutput(vec2<u32>(combiner.coverage_slot), code, fragment_depth(in.clip_position.z));
}

@fragment
fn fs_stamps(in: VsOut) -> @location(0) vec2<u32> {
    replay_alpha(in);
    return vec2<u32>(combiner.coverage_slot);
}

@fragment
fn fs_stamps_decal(in: VsOut) -> @location(0) vec2<u32> {
    decal_test(in);
    replay_alpha(in);
    return vec2<u32>(combiner.coverage_slot);
}

@fragment
fn fs_delta(in: VsOut) -> DeltaOutput {
    let code = delta_code(incoming_delta(in));
    replay_alpha(in);
    return DeltaOutput(code, fragment_depth(in.clip_position.z));
}

@fragment
fn fs_delta_decal(in: VsOut) -> DeltaOutput {
    let code = delta_code(incoming_delta(in));
    decal_test(in);
    replay_alpha(in);
    return DeltaOutput(code, fragment_depth(in.clip_position.z));
}
