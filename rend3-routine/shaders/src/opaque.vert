#version 440

#extension GL_GOOGLE_include_directive : require

#include "structures.glsl"

layout(location = 0) in vec3 i_position;
layout(location = 1) in vec3 i_normal;
layout(location = 2) in vec3 i_tangent;
layout(location = 3) in vec2 i_coords0;
layout(location = 4) in vec2 i_coords1;
layout(location = 5) in vec4 i_color;
layout(location = 6) in uint i_material;
#ifdef GPU_MODE
layout(location = 7) in uint i_object_idx;
#endif

layout(location = 0) out vec4 o_view_position;
layout(location = 1) out vec3 o_normal;
layout(location = 2) out vec3 o_tangent;
layout(location = 3) out vec2 o_coords0;
layout(location = 4) out vec2 o_coords1;
layout(location = 5) out vec4 o_color;
layout(location = 6) flat out uint o_material;

layout(set = 0, binding = 3) uniform UniformBuffer {
    UniformData uniforms;
};
layout(set = 1, binding = 0, std430) restrict readonly buffer ObjectOutputDataBuffer {
    ObjectOutputData object_output[];
};
#ifdef GPU_MODE
layout(set = 1, binding = 1, std430) readonly buffer MaterialBuffer {
    GPUMaterialData materials[];
};
#endif
#ifdef CPU_MODE
layout(set = 2, binding = 10) uniform TextureData {
    CPUMaterialData material;
};
#endif

void main() {
    #ifdef GPU_MODE
    GPUMaterialData material = materials[i_material];
    #endif

    #ifdef CPU_MODE
    uint object_idx = gl_InstanceIndex;
    #else
    uint object_idx = i_object_idx;
    #endif

    ObjectOutputData data = object_output[object_idx];

    o_material = data.material_idx;

    o_view_position = data.model_view * vec4(i_position, 1.0);

    o_normal = normalize(mat3(data.model_view) * (data.inv_squared_scale * i_normal));

    o_tangent = normalize(mat3(data.model_view) * (data.inv_squared_scale * i_tangent));

    o_color = i_color;

    o_coords0 = i_coords0;
    o_coords1 = i_coords1;

    #ifdef BAKING
    vec2 coord1_adj = vec2(material.uv_transform1 * vec3(i_coords1, 1.0));
    gl_Position = vec4(coord1_adj * 2.0 - 1.0, 0.0, 1.0);
    #else
    gl_Position = data.model_view_proj * vec4(i_position, 1.0);
    #endif
}
