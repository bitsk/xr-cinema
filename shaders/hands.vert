#version 450

layout(location=0) in vec3 position;
layout(location=1) in vec3 normal;
layout(location=2) in vec4 finish;
layout(location=3) in uint part;
layout(location=0) out vec3 surfaceNormal;
layout(location=1) out vec3 eyeVector;
layout(location=2) out vec4 material;
layout(location=3) out vec2 sprite;
layout(location=4) flat out uint kind;
layout(location=5) flat out uint flags;
layout(location=6) flat out vec3 display; // handedness, sRGB target, ray length (m)

layout(push_constant) uniform HandView {
    vec4 eyeRotation;
    vec4 eyeOrigin;   // xyz, hand sign
    vec4 tangents;    // left, right, up, down
    vec4 gripRotation;
    vec4 gripOrigin;  // xyz, trigger travel
    vec4 rayOrigin;   // xyz, packed switches
    vec4 rayDirection;// xyz, length
    vec4 controls;    // joystick xy, spare, sRGB target
} pc;

vec3 rotate(vec4 q, vec3 v) {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}
vec3 rotateX(vec3 p, float a) {
    float s=sin(a), c=cos(a);
    return vec3(p.x, c*p.y-s*p.z, s*p.y+c*p.z);
}
bool flag(uint mask) { return (flags & mask) != 0u; }
void main() {
    flags = uint(pc.rayOrigin.w);
    kind = part;
    display = vec3(pc.eyeOrigin.w, pc.controls.w, pc.rayDirection.w);
    material = finish;
    surfaceNormal = normal;
    sprite = position.xy;
    eyeVector = vec3(0.0, 0.0, 1.0);
    bool beam = part == 10u, cursor = part == 11u;
    bool hidden = (!beam && !cursor && !flag(1u)) || (beam && !flag(2u))
        || (cursor && !(flag(2u) && flag(4u) && flag(8u)))
        || ((part == 6u || part == 7u) && display.x < 0.0)
        || ((part == 8u || part == 9u) && display.x > 0.0);
    if (hidden) {
        gl_Position = vec4(2.0, 0.0, 0.0, 1.0);
        return;
    }
    vec3 world;
    if (beam) {
        // Eye-facing strip around the *same* 3D segment for both eyes.
        // The width grows with distance to remain legible at the far panel.
        vec3 dir = pc.rayDirection.xyz;
        sprite.y = pc.rayDirection.w * position.y; // physical travel along the ray, metres
        vec3 mid = pc.rayOrigin.xyz + dir * sprite.y;
        vec3 side = cross(dir, pc.eyeOrigin.xyz - mid);
        vec3 eyeRight = rotate(pc.eyeRotation, vec3(1.0, 0.0, 0.0));
        side = dot(side,side) > 1e-9 ? normalize(side) : eyeRight;
        float radius = 0.00145 + length(mid - pc.eyeOrigin.xyz) * 0.00185;
        if (!flag(4u)) radius *= 0.79;
        world = mid + side * (position.x * radius);
    } else if (cursor) {
        vec3 hit = pc.rayOrigin.xyz + pc.rayDirection.xyz * pc.rayDirection.w;
        float radius = clamp(length(hit-pc.eyeOrigin.xyz) * 0.0072, 0.010, 0.055);
        radius *= flag(16u) ? 0.78 : 1.0;
        // The control surface is a world-space XY plane. Keeping the reticle
        // coplanar places it on the widget under head translation as well.
        world = hit + vec3(position.xy * radius, 0.00035);
    } else {
        vec3 p = position, n = normal;
        if (part == 1u) {
            vec3 hinge = vec3(0.0, 0.018, -0.046);
            float angle = -0.30 * pc.gripOrigin.w;
            p = hinge + rotateX(p-hinge, angle);
            n = rotateX(n,angle);
            material.rgb += vec3(0.055,0.085,0.10) * pc.gripOrigin.w;
        } else if (part == 2u && flag(32u)) {
            p.x += 0.0025;
        } else if (part == 3u) {
            float lever = clamp((p.y - 0.041) / 0.012, 0.0, 1.0);
            p.xz += vec2(pc.controls.x*display.x, -pc.controls.y) * (0.005*lever);
        } else {
            bool first = part == 4u || part == 6u || part == 8u;
            bool second = part == 5u || part == 7u || part == 9u;
            if ((first && flag(64u)) || (second && flag(128u))) {
                p -= vec3(0.0,0.964,0.267) * 0.0020;
                if (part == 4u || part == 5u) material.rgb += vec3(0.04,0.11,0.15);
            }
        }
        p.x *= display.x;
        n.x *= display.x;
        world = pc.gripOrigin.xyz + rotate(pc.gripRotation,p);
        surfaceNormal = rotate(pc.gripRotation,n);
        eyeVector = pc.eyeOrigin.xyz - world;
    }
    vec3 eye = rotate(vec4(-pc.eyeRotation.xyz, pc.eyeRotation.w),world-pc.eyeOrigin.xyz);
    float w = -eye.z;
    vec4 f = pc.tangents;
    // Inverse of the cinema shader's per-eye asymmetric-FOV ray construction.
    gl_Position = vec4((2.0*eye.x - w*(f.x+f.y))/(f.y-f.x),
                       (2.0*eye.y - w*(f.z+f.w))/(f.w-f.z),
                       w-0.015, w);
}
