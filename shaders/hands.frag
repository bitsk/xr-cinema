#version 450
layout(location=0) in vec3 surfaceNormal;
layout(location=1) in vec3 eyeVector;
layout(location=2) in vec4 material;
layout(location=3) in vec2 sprite;
layout(location=4) flat in uint kind;
layout(location=5) flat in uint flags;
layout(location=6) flat in vec3 display;
layout(location=0) out vec4 fragment;

vec3 target(vec3 rgb) {
    rgb=clamp(rgb,0.0,1.0);
    if (display.y < 0.5) return rgb;
    return mix(rgb/12.92, pow((rgb+0.055)/1.055,vec3(2.4)),
               greaterThan(rgb,vec3(0.04045)));
}
bool flag(uint mask) { return (flags & mask) != 0u; }
void main() {
    vec3 accent = display.x < 0.0 ? vec3(0.32,0.85,0.93) : vec3(0.39,0.70,1.0);
    bool pressed = flag(16u), aimed = flag(8u);
    vec3 color;
    float alpha = 1.0;
    if (kind == 10u) {
        float edge=abs(sprite.x);
        float aa = clamp(fwidth(sprite.x)*0.48,0.02,0.42);
        float beam=1.0-smoothstep(0.94-aa,0.94+aa,edge);
        float core=1.0-smoothstep(0.18-aa,0.38+aa,edge);
        float halo=1.0-smoothstep(0.31,0.83,edge);
        // Start visibly on the glass. This 2.8 mm transition stays fixed even
        // when the controller targets a panel several metres from the hand.
        float envelope = mix(0.94, 1.0, smoothstep(0.0, 0.0028, sprite.y));
        if (!aimed) {
            float fadeLength = min(0.25, display.z * 0.45);
            envelope *= 1.0 - smoothstep(display.z - fadeLength, display.z, sprite.y);
        }
        color = mix(vec3(0.09,0.17,0.25),accent,halo);
        color = mix(color,pressed ? vec3(0.97,1.0,1.0) : mix(accent,vec3(1.0),0.68),core);
        alpha = beam*(0.28+0.59*halo+0.13*core)*envelope;
        if (!aimed) alpha *= 0.68;
        if (!flag(4u)) alpha *= 0.54;
    } else if (kind == 11u) {
        float r=length(sprite);
        float aa=clamp(fwidth(r)*0.58,0.018,0.19);
        float band=abs(r-0.67);
        float outline=1.0-smoothstep(0.18-aa,0.18+aa,band);
        float ring=1.0-smoothstep(0.087-aa,0.087+aa,band);
        float dotShadow=1.0-smoothstep(0.23-aa,0.23+aa,r);
        float dotFace=1.0-smoothstep(0.115-aa,0.115+aa,r);
        float ink=max(ring,dotFace);
        color=mix(vec3(0.075,0.14,0.20),mix(accent,vec3(1.0),pressed?0.83:0.60),ink);
        alpha=max(outline,dotShadow)*0.95;
    } else {
        vec3 n=normalize(surfaceNormal), v=normalize(eyeVector);
        // A large overhead key and a rear strip reveal the curvature even on
        // the wrist-facing side of the grip, away from the cinema screen.
        vec3 light=normalize(vec3(-0.60,0.66,0.55));
        vec3 fill=normalize(vec3(0.84,0.24,-0.48));
        vec3 h=normalize(v+light);
        float rim=pow(1.0-max(0.0,dot(n,v)),2.4);
        float bounce=0.15*(n.y*0.5+0.5);
        float spec=pow(max(0.0,dot(n,h)),mix(13.0,58.0,material.a));
        color=material.rgb*(0.40+bounce+0.50*max(0.0,dot(n,light))+0.15*max(0.0,dot(n,fill)));
        float softbox=smoothstep(0.77,0.96,dot(reflect(-v,n),normalize(vec3(0.76,0.25,0.60))));
        color+=vec3(0.84,0.91,1.0)*(spec*(0.065+0.25*material.a)+rim*(0.065+0.08*material.a)
                                      +softbox*0.11*material.a);
        if (kind==12u) {
            // Shallow chevrons moulded into the rear thumb pad, in metres.
            float phase = sprite.y + 0.27*abs(sprite.x);
            float ridge=abs(fract(phase/0.008)-0.5)*0.008;
            float aa=clamp(fwidth(phase),0.0001,0.0015);
            float mask=smoothstep(-0.071,-0.061,sprite.y)*(1.-smoothstep(-0.032,-0.019,sprite.y));
            color*=1.0-mask*0.24*(1.0-smoothstep(0.00018,0.0004+aa,ridge));
        }
        if (material.a>1.0) color=mix(accent,vec3(0.93,1.0,1.0),0.20+0.22*float(pressed));
        if (kind==13u) color = flag(2u)
            ? mix(accent, vec3(0.97,1.0,1.0), flag(4u) ? 0.75 : 0.38)
            : vec3(0.20,0.32,0.39);
    }
    if (alpha < 0.004) discard;
    fragment = vec4(target(color),alpha);
}
