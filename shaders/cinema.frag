#version 450

layout(location = 0) in vec2 viewUV;
layout(location = 0) out vec4 fragment;
layout(binding = 0) uniform sampler2D video;
layout(binding = 1) uniform sampler2D hud;
layout(push_constant) uniform View {
    vec4 rotation;
    vec4 origin;      // eye position in cinema space, HUD opacity
    vec4 tangentFov;  // left, right, up, down
    ivec4 mode;       // projection, layout, source eye, cinema enabled
    vec4 display;     // per-eye aspect, reserved, reserved, sRGB target
    vec4 panel;       // width, height, center y, center z (zero width hides)
} pc;

const float PI = 3.14159265359;

vec3 sceneColor(vec3 srgb) {
    if (pc.display.w < 0.5) return srgb;
    return mix(srgb / 12.92, pow((srgb + 0.055) / 1.055, vec3(2.4)),
               greaterThan(srgb, vec3(0.04045)));
}

// UVs use a top-left origin in the decoded frame, HUD and controller hit test.
vec3 sampleVideo(vec2 uv) {
    vec2 scale = vec2(1.0);
    if (pc.mode.y == 1) scale.x = 0.5;
    if (pc.mode.y == 2) scale.y = 0.5;
    vec2 margin = 0.5 / vec2(textureSize(video, 0));
    vec2 start = (1.0 - scale) * float(pc.mode.z);
    return texture(video, start + clamp(uv * scale, margin, max(margin, scale - margin))).rgb;
}

vec3 auditorium(vec3 o, vec3 d) {
    // First intersection with one of six interior surfaces.
    vec3 positive = vec3(5.6, 3.8, 4.6);
    vec3 negative = vec3(-5.6, -1.55, -6.8);
    vec3 t = (mix(negative, positive, greaterThan(d, vec3(0.0))) - o)
               / mix(min(d, vec3(-1e-6)), max(d, vec3(1e-6)), greaterThan(d, vec3(0.0)));
    float hit = min(t.x, min(t.y, t.z));
    vec3 p = o + hit * d;
    vec3 surface;
    if (hit == t.y && d.y < 0.0) {
        float aisle = smoothstep(0.16, 0.20, abs(abs(p.x) - 2.95));
        surface = mix(vec3(0.17, 0.15, 0.18), vec3(0.095, 0.095, 0.14), aisle);
        float stepLight = 1.0 - smoothstep(0.014, 0.026, abs(abs(p.x) - 3.15));
        surface += vec3(0.47, 0.33, 0.20) * stepLight;
    } else if (hit == t.y) {
        float strip = 1.0 - smoothstep(0.018, 0.045, abs(abs(p.x) - 3.6));
        surface = vec3(0.075, 0.09, 0.13) + strip * vec3(0.38, 0.52, 0.66);
    } else {
        float along = hit == t.x ? p.z : p.x;
        float rib = abs(fract(along * 0.67) - 0.5);
        float edge = 1.0 - smoothstep(0.008, 0.022, rib);
        float band = exp(-abs(p.y - 0.75) * 1.8);
        surface = mix(vec3(0.085, 0.10, 0.15), vec3(0.15, 0.18, 0.24), band);
        surface += vec3(0.19, 0.30, 0.43) * edge * band;
        float sconce = (1.0 - smoothstep(0.018, 0.035, rib))
                       * (1.0 - smoothstep(0.23, 0.27, abs(p.y - 0.75)));
        surface += vec3(0.54, 0.40, 0.24) * sconce;
    }
    // Raked rows of upholstered backs, with gaps for the two aisles.
    if (abs(d.z) > 0.001 && d.y < 0.2) {
        for (int i = 0; i < 7; ++i) {
            float row = float(i);
            float dist = (-4.4 + row * 1.40 - o.z) / d.z;
            if (dist < 0.0 || dist >= hit) continue;
            vec2 at = o.xy + dist * d.xy;
            if (abs(at.x) > 5.05 || abs(abs(at.x) - 2.95) < 0.33) continue;
            vec2 seat = vec2(abs(mod(at.x + 0.46, 0.92) - 0.46),
                             at.y - (-1.25 + 0.125 * row));
            vec2 q = abs(vec2(seat.x, seat.y)) - vec2(0.32, 0.31);
            float edge = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.096;
            if (edge > 0.0) continue;
            float seam = 1.0 - smoothstep(0.003, 0.014, abs(edge + 0.045));
            float top = smoothstep(-0.28, 0.40, seat.y);
            surface = mix(vec3(0.095, 0.08, 0.125), vec3(0.245, 0.17, 0.23), top);
            surface += vec3(0.07, 0.045, 0.055) * seam;
            hit = dist;
        }
    }
    return surface;
}

vec3 scene(vec3 o, vec3 d) {
    if (pc.mode.x != 0) {
        float yaw = atan(d.x, -d.z);
        float halfSpan = pc.mode.x == 1 ? PI * 0.5 : PI;
        if (abs(yaw) <= halfSpan) {
            return sampleVideo(vec2(0.5 + yaw / (2.0 * halfSpan),
                                    0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI));
        }
    } else if (d.z < -0.001) {
        float t = (-6.45 - o.z) / d.z;
        vec2 screen = (o.xy + t * d.xy) - vec2(0.0, 0.82);
        // Fit portrait, standard and cinemascope video into the same proscenium.
        float halfHeight = min(2.05, 4.45 / pc.display.x);
        vec2 size = vec2(halfHeight * pc.display.x, halfHeight);
        if (t > 0.0 && all(lessThanEqual(abs(screen), size)))
            return sampleVideo(vec2(0.5) + vec2(0.5, -0.5) * screen / size);
        if (t > 0.0 && all(lessThanEqual(abs(screen), size + 0.065)))
            return sceneColor(vec3(0.042, 0.048, 0.065));
    }
    return sceneColor(pc.mode.w == 1 ? auditorium(o, d) : vec3(0.011, 0.017, 0.028));
}

void main() {
    vec3 ray = normalize(vec3(mix(pc.tangentFov.x, pc.tangentFov.y, viewUV.x),
                              mix(pc.tangentFov.z, pc.tangentFov.w, viewUV.y), -1.0));
    vec4 q = pc.rotation;
    vec3 d = ray + 2.0 * cross(q.xyz, cross(q.xyz, ray) + q.w * ray);
    vec3 o = pc.origin.xyz;
    vec4 overlay = vec4(0.0);
    if (pc.panel.x > 0.0 && d.z < -0.001) {
        float t = (pc.panel.w - o.z) / d.z;
        vec2 uv = vec2(0.5) + (o.xy + t * d.xy - vec2(0.0, pc.panel.z))
                             * vec2(1.0, -1.0) / pc.panel.xy;
        if (t > 0.0 && all(greaterThanEqual(uv, vec2(0.0))) && all(lessThanEqual(uv, vec2(1.0)))) {
            overlay = texture(hud, uv);
            overlay.a *= pc.origin.w;
        }
    }
    vec3 color = overlay.rgb;
    if (overlay.a < 0.999) color = mix(scene(o, d), color, overlay.a);
    fragment = vec4(color, 1.0);
}
