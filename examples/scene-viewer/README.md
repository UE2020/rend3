# scene-viewer

gltf (and glb) loader and viewer using the [rend3](https://crates.io/crates/rend3) rendering engine.

## Default Scene

To download the default scene:

```bash
# On windows, make sure to type curl.exe to get real curl, not the alias in powershell.
# On *nix, just type `curl`.
curl.exe https://cdn.cwfitz.com/scenes/rend3-default-scene.tar -o ./examples/scene-viewer/resources/rend3-default-scene.tar
tar xf ./examples/scene-viewer/resources/rend3-default-scene.tar -C ./examples/scene-viewer/resources
```

The source of the default scene is available here:

https://cdn.cwfitz.com/scenes/rend3-default-scene.blend

Default scene, exposed through glTF:

![](https://raw.githubusercontent.com/BVE-Reborn/rend3/trunk/examples/scene-viewer/screenshot.jpg)

Exported Unity Scene through glTF:

![](https://raw.githubusercontent.com/BVE-Reborn/rend3/trunk/examples/scene-viewer/scifi-base.jpg)

Emerald-Square from [Nvidia ORCA](https://developer.nvidia.com/orca) exported to GLTF with blender:

![](https://raw.githubusercontent.com/BVE-Reborn/rend3/trunk/examples/scene-viewer/emerald-square.jpg)
