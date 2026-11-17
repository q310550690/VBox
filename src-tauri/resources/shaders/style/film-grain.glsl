// VBox built-in style shader: fine film grain (static, spatially stable).
// MIT, authored for VBox.
//!HOOK MAIN
//!BIND HOOKED
//!DESC VBox Film Grain

float vbox_grain(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    float noise = (vbox_grain(HOOKED_pos * HOOKED_size) - 0.5) * 0.045;
    return vec4(clamp(c.rgb + noise, 0.0, 1.0), c.a);
}
