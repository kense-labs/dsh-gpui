This is the crates.io source of `gpui-pre-wgpu` 0.3.10, based on Zed revision
`08cbb88a152c3751a43e853425a70e9f631a5918` and distributed under Apache-2.0.

KWork adds a backdrop pass to the scene renderer for translucent menus. The
public `backdrop_quad` constructor and the interpretation of its borderless
quad are kept here together. Ordinary scenes keep the upstream direct rendering
path. When updating GPUI, reapply the changes in `gpui_wgpu.rs`,
`wgpu_renderer.rs`, `backdrop.rs`, `backdrop.wgsl` and the shadow shader
in `shaders.wgsl`.
