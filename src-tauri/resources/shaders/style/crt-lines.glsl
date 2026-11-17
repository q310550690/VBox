// VBox built-in style shader: subtle CRT scanlines / phosphor mask.
// MIT, authored for VBox.
//!HOOK MAIN
//!BIND HOOKED
//!DESC VBox CRT Scanlines

vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    float scan = 0.92 + 0.08 * sin(HOOKED_pos.y * HOOKED_size.y * 1.6);
    c.rgb *= scan;
    return c;
}
