#version 450

layout(location = 0) out vec3 viewRay;
layout(push_constant) uniform View {
    vec4 rotation;
    vec4 origin;
    vec4 tangentFov;
} pc;
void main() {
    vec2 viewUV = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    // The eye-plane ray is affine across the full-screen triangle and
    // quaternion rotation is linear. Normalize after interpolation.
    vec3 ray = vec3(mix(pc.tangentFov.x, pc.tangentFov.y, viewUV.x),
                    mix(pc.tangentFov.z, pc.tangentFov.w, viewUV.y), -1.0);
    vec4 q = pc.rotation;
    viewRay = ray + 2.0 * cross(q.xyz, cross(q.xyz, ray) + q.w * ray);
    gl_Position = vec4(viewUV * 2.0 - 1.0, 0.0, 1.0);
}
