// VBox built-in style shader: film look (contrast / warmth / slight saturation).
// MIT, authored for VBox.
//!HOOK MAIN
//!BIND HOOKED
//!DESC VBox Film Look

vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    // Slight S-curve contrast, then a gentle warm tint.
    c.rgb = clamp((c.rgb - 0.5) * 1.06 + 0.5, 0.0, 1.0);
    float luma = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    c.rgb = mix(vec3(luma), c.rgb, 1.08);
    c.r = clamp(c.r + 0.015, 0.0, 1.0);
    c.b = clamp(c.b - 0.008, 0.0, 1.0);
    return c;
}
