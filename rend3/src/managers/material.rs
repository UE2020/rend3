use crate::{
    managers::{ObjectManager, TextureManager},
    mode::ModeData,
    types::{MaterialHandle, TextureHandle},
    util::{
        bind_merge::{BindGroupBuilder, BindGroupLayoutBuilder},
        buffer::WrappedPotBuffer,
        math::round_up_pot,
        registry::ArchitypicalErasedRegistry,
        typedefs::FastHashMap,
    },
    RendererMode,
};
use list_any::VecAny;
use rend3_types::{Material, MaterialTag, RawMaterialHandle, RawObjectHandle};
use std::{
    any::TypeId,
    num::{NonZeroU32, NonZeroU64},
};
use wgpu::{
    util::{BufferInitDescriptor, DeviceExt},
    BindGroup, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, Buffer,
    BufferBinding, BufferBindingType, BufferUsages, Device, Queue, ShaderStages, TextureSampleType,
    TextureViewDimension,
};

const TEXTURE_MASK_SIZE: u32 = 4;

pub struct InternalMaterial {
    pub bind_group: ModeData<BindGroup, ()>,
    pub material_buffer: ModeData<Buffer, ()>,
    pub key: u64,
    /// Handles of all objects
    pub objects: Vec<RawObjectHandle>,
}

#[allow(clippy::type_complexity)]
struct PerTypeInfo {
    bgl: ModeData<BindGroupLayout, ()>,
    data_size: u32,
    texture_count: u32,
    write_gpu_materials_fn: fn(&mut [u8], &VecAny, &mut (dyn FnMut(&TextureHandle) -> NonZeroU32 + '_)) -> usize,
    get_material_key: fn(vec_any: &VecAny, usize) -> MaterialKeyPair,
}

/// Key which determine's an object's archetype.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct MaterialKeyPair {
    /// TypeId of the material the object uses.
    pub ty: TypeId,
    /// Per-material data.
    pub key: u64,
}

struct BufferRange {
    offset: u64,
    size: NonZeroU64,
}

/// Manages materials and their associated BindGroups in CPU modes.
pub struct MaterialManager {
    bg: FastHashMap<TypeId, ModeData<(), BufferRange>>,
    type_info: FastHashMap<TypeId, PerTypeInfo>,

    buffer: ModeData<(), WrappedPotBuffer>,

    registry: ArchitypicalErasedRegistry<MaterialTag, InternalMaterial>,
}

impl MaterialManager {
    pub fn new(device: &Device, mode: RendererMode) -> Self {
        profiling::scope!("MaterialManager::new");

        let buffer = mode.into_data(
            || (),
            || WrappedPotBuffer::new(device, 0, 16, BufferUsages::STORAGE, Some("material buffer")),
        );

        let registry = ArchitypicalErasedRegistry::new();

        Self {
            bg: FastHashMap::default(),
            type_info: FastHashMap::default(),
            buffer,
            registry,
        }
    }

    pub fn allocate(&self) -> MaterialHandle {
        self.registry.allocate()
    }

    pub fn ensure_archetype<M: Material>(&mut self, device: &Device, mode: RendererMode) {
        self.ensure_archetype_inner::<M>(device, mode);
    }

    fn ensure_archetype_inner<M: Material>(&mut self, device: &Device, mode: RendererMode) -> &mut PerTypeInfo {
        profiling::scope!("MaterialManager::ensure_archetype");
        let create_bgl = || {
            mode.into_data(
                || {
                    let texture_binding = |idx: u32| BindGroupLayoutEntry {
                        binding: idx as u32,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Float { filterable: true },
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    };

                    let mut entries: Vec<_> = (0..M::TEXTURE_COUNT).map(texture_binding).collect();
                    entries.push(BindGroupLayoutEntry {
                        binding: M::TEXTURE_COUNT,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                (round_up_pot(M::DATA_SIZE, 16) + round_up_pot(TEXTURE_MASK_SIZE, 16)) as _,
                            ),
                        },
                        count: None,
                    });
                    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                        label: Some("cpu material bgl"),
                        entries: &entries,
                    })
                },
                || (),
            )
        };

        self.registry.ensure_archetype::<M>();

        let ty = TypeId::of::<M>();

        self.type_info.entry(ty).or_insert_with(|| PerTypeInfo {
            bgl: create_bgl(),
            data_size: M::DATA_SIZE,
            texture_count: M::TEXTURE_COUNT,
            write_gpu_materials_fn: write_gpu_materials::<M>,
            get_material_key: get_material_key::<M>,
        })
    }

    fn fill_inner<M: Material>(
        &mut self,
        device: &Device,
        mode: RendererMode,
        texture_manager_2d: &mut TextureManager,
        material: &M,
    ) -> InternalMaterial {
        let null_tex = texture_manager_2d.get_null_view();

        let mut translation_fn = texture_manager_2d.translation_fn();

        let type_info = self.ensure_archetype_inner::<M>(device, mode);

        let (bind_group, material_buffer) = if mode == RendererMode::CPUPowered {
            let mut textures = vec![NonZeroU32::new(u32::MAX); M::TEXTURE_COUNT as usize];
            material.to_textures(&mut textures, &mut translation_fn);

            // TODO(material): stack allocation
            let material_uprounded = round_up_pot(M::DATA_SIZE, 16) as usize;
            let actual_size = material_uprounded + round_up_pot(TEXTURE_MASK_SIZE, 16) as usize;
            let mut data = vec![0u8; actual_size as usize];
            material.to_data(&mut data[..M::DATA_SIZE as usize]);

            let mut builder = BindGroupBuilder::new();
            let mut texture_mask = 0_u32;
            for (idx, texture) in textures.into_iter().enumerate() {
                builder.append(BindingResource::TextureView(
                    texture
                        .map(|tex| texture_manager_2d.get_view_from_index(tex))
                        .unwrap_or(null_tex),
                ));
                let enabled = texture.is_some();
                texture_mask |= (enabled as u32) << idx as u32;
            }

            *bytemuck::from_bytes_mut(&mut data[material_uprounded..material_uprounded + 4]) = texture_mask;

            let material_buffer = device.create_buffer_init(&BufferInitDescriptor {
                label: None,
                contents: &data,
                usage: BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            });

            let bind_group =
                builder
                    .append_buffer(&material_buffer)
                    .build(device, None, type_info.bgl.as_ref().as_cpu());

            (ModeData::CPU(bind_group), ModeData::CPU(material_buffer))
        } else {
            (ModeData::GPU(()), ModeData::GPU(()))
        };

        InternalMaterial {
            bind_group,
            material_buffer,
            key: material.object_key(),
            objects: Vec::new(),
        }
    }

    pub fn fill<M: Material>(
        &mut self,
        device: &Device,
        mode: RendererMode,
        texture_manager_2d: &mut TextureManager,
        handle: &MaterialHandle,
        material: M,
    ) {
        let internal = self.fill_inner(device, mode, texture_manager_2d, &material);

        self.registry.insert(handle, material, internal);
    }

    pub fn update<M: Material>(
        &mut self,
        device: &Device,
        mode: RendererMode,
        texture_manager_2d: &mut TextureManager,
        object_manager: &mut ObjectManager,
        handle: &MaterialHandle,
        material: M,
    ) {
        // TODO(material): if this doesn't change archetype, this should do a buffer write cpu side.
        let internal = self.fill_inner(device, mode, texture_manager_2d, &material);

        let archetype_changed = self.registry.update(handle, material, |internal, idx| {
            for object in &internal.objects {
                object_manager.set_material_index(*object, idx)
            }
        });

        if archetype_changed {
            let new_index = self.registry.get_index(handle.get_raw());
            let new_internal = self.registry.get_metadata_mut::<M>(handle.get_raw());

            for object in &new_internal.objects {
                object_manager.set_material_index(*object, new_index);
                object_manager.set_key(
                    *object,
                    MaterialKeyPair {
                        ty: TypeId::of::<M>(),
                        key: internal.key,
                    },
                )
            }
            new_internal.bind_group = internal.bind_group;
            new_internal.material_buffer = internal.material_buffer;
            new_internal.key = internal.key;
        } else {
            let new_internal = self.registry.get_metadata_mut::<M>(handle.get_raw());
            if internal.key != new_internal.key {
                for object in &new_internal.objects {
                    object_manager.set_key(
                        *object,
                        MaterialKeyPair {
                            ty: TypeId::of::<M>(),
                            key: internal.key,
                        },
                    )
                }
                new_internal.key = internal.key;
            }
            new_internal.bind_group = internal.bind_group;
            new_internal.material_buffer = internal.material_buffer;
        }
    }

    pub fn get_material<M: Material>(&self, handle: RawMaterialHandle) -> &M {
        self.registry.get_ref::<M>(handle)
    }

    pub fn get_bind_group_layout_cpu<M: Material>(&self) -> &BindGroupLayout {
        self.type_info[&TypeId::of::<M>()].bgl.as_cpu()
    }

    pub fn add_to_bgl_gpu<M: Material>(bglb: &mut BindGroupLayoutBuilder) {
        bglb.append(
            ShaderStages::VERTEX_FRAGMENT,
            BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new((M::TEXTURE_COUNT * 4 + M::DATA_SIZE) as _),
            },
            None,
        );
    }

    pub fn add_to_bg_gpu<'a, M: Material>(&'a self, bgb: &mut BindGroupBuilder<'a>) {
        let range = self.bg[&TypeId::of::<M>()].as_gpu();
        bgb.append(BindingResource::Buffer(BufferBinding {
            buffer: self.buffer.as_gpu(),
            offset: range.offset,
            size: Some(range.size),
        }));
    }

    pub fn get_internal_material_full<M: Material>(&self, handle: RawMaterialHandle) -> (&M, &InternalMaterial) {
        self.registry.get_ref_full::<M>(handle)
    }

    pub fn get_internal_material_full_by_index<M: Material>(&self, index: usize) -> (&M, &InternalMaterial) {
        self.registry.get_ref_full_by_index::<M>(index)
    }

    pub fn get_material_key_and_objects(
        &mut self,
        handle: RawMaterialHandle,
    ) -> (MaterialKeyPair, &mut Vec<RawObjectHandle>) {
        let index = self.registry.get_index(handle);
        let ty = self.registry.get_type_id(handle);
        let type_info = &self.type_info[&ty];
        let arch = self.registry.get_archetype_mut(ty);

        let key_pair = (type_info.get_material_key)(&arch.vec, index);

        (key_pair, &mut arch.non_erased[index].inner.objects)
    }

    pub fn get_objects(&mut self, handle: RawMaterialHandle) -> &mut Vec<RawObjectHandle> {
        let index = self.registry.get_index(handle);
        let ty = self.registry.get_type_id(handle);
        let arch = self.registry.get_archetype_mut(ty);

        &mut arch.non_erased[index].inner.objects
    }

    pub fn get_internal_index(&self, handle: RawMaterialHandle) -> usize {
        self.registry.get_index(handle)
    }

    pub fn ready(
        &mut self,
        device: &Device,
        queue: &Queue,
        object_manager: &mut ObjectManager,
        texture_manager: &TextureManager,
    ) {
        profiling::scope!("Material Ready");
        self.registry.remove_all_dead(|internal, idx| {
            for object in &internal.objects {
                object_manager.set_material_index(*object, idx);
            }
        });

        if let ModeData::GPU(ref mut buffer) = self.buffer {
            profiling::scope!("Update GPU Material Buffer");
            let mut translate_texture = texture_manager.translation_fn();

            let bytes: usize = self
                .registry
                .archetype_lengths()
                .map(|(ty, len)| {
                    let type_info = &self.type_info[&ty];

                    len.max(1)
                        * (round_up_pot(type_info.texture_count * 4, 16) + round_up_pot(type_info.data_size, 16))
                            as usize
                })
                .sum::<usize>();

            buffer.ensure_size(device, bytes as u64);

            let mut data = vec![0u8; bytes];

            let mut offset = 0_usize;
            for (ty, archetype) in self.registry.archetypes_mut() {
                let type_info = &self.type_info[&ty];

                let size = (type_info.write_gpu_materials_fn)(&mut data[offset..], archetype, &mut translate_texture);
                let size = size.max(16);

                self.bg.insert(
                    ty,
                    ModeData::GPU(BufferRange {
                        offset: offset as u64,
                        size: NonZeroU64::new(size as u64).unwrap(),
                    }),
                );

                offset += size;
            }

            // TODO(material): I know the size before hand, we could elide this cpu side copy
            buffer.write_to_buffer(device, queue, bytemuck::cast_slice(&data));
        }
    }
}

fn write_gpu_materials<M: Material>(
    dest: &mut [u8],
    vec_any: &VecAny,
    translation_fn: &mut (dyn FnMut(&TextureHandle) -> NonZeroU32 + '_),
) -> usize {
    let materials = vec_any.downcast_slice::<M>().unwrap();

    let mut offset = 0_usize;

    let texture_bytes = (M::TEXTURE_COUNT * 4) as usize;
    let mat_size = round_up_pot(texture_bytes, 16);
    let data_size = round_up_pot(M::DATA_SIZE, 16) as usize;

    for mat in materials {
        // If we have no textures, we should skip this operation as the cast_slice_mut will fail
        if mat_size != 0 {
            let texture_slice = bytemuck::cast_slice_mut(&mut dest[offset..offset + texture_bytes]);
            mat.to_textures(texture_slice, translation_fn);

            offset += mat_size;
        }

        // If we have no data, skip calling the material for data.
        if data_size != 0 {
            mat.to_data(&mut dest[offset..offset + M::DATA_SIZE as usize]);

            offset += data_size;
        }
    }

    offset.max(mat_size + data_size)
}

fn get_material_key<M: Material>(vec_any: &VecAny, index: usize) -> MaterialKeyPair {
    let materials = vec_any.downcast_slice::<M>().unwrap();

    let key = materials[index].object_key();

    MaterialKeyPair {
        ty: TypeId::of::<M>(),
        key,
    }
}
