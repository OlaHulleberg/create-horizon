fn pipeline_fragment(input: PipelineInput) -> vec4<f32> {
    return pipeline_texture(input.uv) * input.color;
}
