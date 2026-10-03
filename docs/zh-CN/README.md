> 全部中文文档的目录见 [`文档索引.md`](文档索引.md)。
>
> 本文是 [`README.md`](../../README.md) 的中文翻译。英文版是权威版本，如有歧义以英文版为准。

# Noxel

一个轻量、模块化、高性能的 **3D 俯视角像素风 RPG 引擎**，用 Rust 编写，**不依赖任何第三方库**。

```text
$ cargo run -p town-demo                # generate a world, walk a town, write PNG frames
$ cargo test --workspace                # 1475 tests, a few seconds, no display needed
$ cargo run -p noxel-gen -- generate    # regenerate every asset in examples/town-demo/assets
```

---

## 它是什么

Noxel 从俯视相机渲染一个 3D 世界，内部以像素风分辨率（默认 320x180）绘制，再放大到窗口。它面向这样的游戏：世界很大、相机离得很远，而且有大量东西在移动——俯视角 RPG、殖民地模拟、战术游戏，以及任何需要渲染人群的游戏。

| 需求 | 实现位置 |
|---|---|
| 像素风 3D 俯视渲染器 | [`noxel-render`](../../crates/noxel-render) —— 基于 tile 的软件光栅化器 + 光线追踪器 |
| 轻量、跨平台 | 零依赖、无 `unsafe`、纯 `std` |
| 大世界的生成与运行时 | [`noxel-world`](../../crates/noxel-world) —— 确定性生成、区块流式加载 |
| 便于搭建城镇与道路 | [`noxel-world`](../../crates/noxel-world) —— 道路格网、城镇规划、程序化建筑、预制体 |
| 精确的相机运动 | [`noxel-camera`](../../crates/noxel-camera) —— 死区、前瞻、像素级吸附、抖动 |
| 物体遮挡视野时自动透明 | [`noxel-visibility`](../../crates/noxel-visibility) —— 由射线驱动的相机遮挡淡出 |
| 剔除被遮挡的物体 | [`noxel-visibility`](../../crates/noxel-visibility) —— 视锥、距离、尺寸、遮挡 |
| 支持丰富玩家交互的物理 | [`noxel-physics`](../../crates/noxel-physics) —— 刚体、SAT、角色控制器、射线查询 |
| 光栅化与光线追踪并存 | [`noxel-render`](../../crates/noxel-render) —— 同一场景上的三种模式 |
| 同时存在大量 NPC | [`noxel-npc`](../../crates/noxel-npc) —— 人群分层、流场寻路、转向 |
| 模块化、可扩展 | 每个子系统一个 crate，一个 `Renderer` trait，一个 `Plugin` trait |
| 用详尽的文档代替 UI | `docs/` —— 总览、指南、API、10 篇 ADR |

---

## 快速开始

**运行预构建的示例**（`dist/` 构建好之后就不需要 Rust 工具链了）：

```bash
./scripts/build-dist.sh          # builds dist/town-demo + dist/noxel-gen + dist/assets
cd dist && ./town-demo --frames 600
```

`dist/` 自包含且可迁移：复制到任何地方都能跑。它包含两个可执行文件、它们需要的资源，以及一个 `README.txt`。

**从源码运行**（Rust 1.85+，edition 2024）：

```bash
git clone <this repo> && cd noxel
cargo run -p town-demo -- --frames 600
```

两种方式都一样，打开 `frames/frame_000599.png`。你应该会看到一个村庄：广场、街道、几十栋建筑、山坡上的树木、一条河，以及一个沿脚本路线行走的角色，相机跟在后面。

在同一个世界上切换渲染器：

```bash
cargo run -p town-demo -- --mode hybrid     # raster primary + ray-traced shadows and AO
cargo run -p town-demo -- --mode raytrace   # full ray tracing (slow, beautiful)
```

---

## 五分钟导览

基于 Noxel 的引擎就是一个 `App` 加上若干插件。下面这个例子是完整的：

```rust
use noxel_app::{App, AppConfig, Plugin};
use noxel_core::math::{Transform, Vec3};
use noxel_render::mesh::Mesh;
use noxel_render::material::Material;
use noxel_render::Color;

struct Player;

impl Plugin for Player {
    fn name(&self) -> &str { "player" }

    fn build(&mut self, app: &mut App) {
        let mesh = app.scene_mut().add_mesh(Mesh::capsule(0.35, 1.7));
        let material = app.scene_mut().add_material(Material::lit("player", Color::WHITE));
        let handle = app.scene_mut().spawn("player", mesh, material, Transform::IDENTITY);
        app.context.focus = Vec3::ZERO;
        let _ = handle;
    }
}

fn main() {
    let mut app = App::new(AppConfig::default()).unwrap();
    app.add_plugin(Player);
    let report = app.run_headless(300);
    println!("{}", report.summary());
}
```

`app.run_headless(n)` 在没有窗口的情况下渲染真实的帧。如果主循环由你自己掌控，`App::step(dt)` 推进一帧；`docs/guides/windowing.md` 演示了如何接入 `winit` 或 `SDL2` 来呈现帧缓冲。

---

## 架构

```text
                                    noxel-app
                        (App, Plugin, fixed-step frame loop)
                                       |
        +--------------+---------------+---------------+--------------+
        |              |               |               |              |
   noxel-npc     noxel-world    noxel-physics   noxel-camera   noxel-debug
   (crowds,      (generation,   (bodies,        (rig, shake,   (stats,
    steering,     streaming,     SAT, queries,   zones,         overlays,
    schedules)    roads, towns)  character)      pixel-perfect) dumping)
        |              |               |               |              |
        +--------------+-------+-------+-------+-------+--------------+
                               |               |
                        noxel-visibility   noxel-render
                        (frustum, occlusion, (raster + raytrace,
                         LOD, camera fades)  framebuffer, scene)
                               |               |
                            noxel-ecs      noxel-asset
                            (entities,     (PNG, JSON, atlas,
                             components,    formats, hot reload)
                             scheduler)
                               |               |
                               +-------+-------+
                                       |
                                  noxel-core
                    (math, rng, time, jobs, pools, spatial, events)
```

依赖关系**只朝一个方向**。`noxel-core` 不依赖任何东西；`noxel-app` 依赖所有东西。添加一条反向的依赖边是设计错误，而不是可以绕过去的编译错误。

| Crate | 用途 |
|---|---|
| `noxel-core` | 数学、确定性 RNG、固定时钟、任务池、slot map、空间结构、事件 |
| `noxel-ecs` | 稀疏集实体与组件、分阶段调度器 |
| `noxel-asset` | PNG 编解码器、JSON、图像、纹理、图集、数据格式、热重载资源库 |
| `noxel-render` | 帧缓冲、网格、材质、光源、场景、光栅化器、光线追踪器、调试叠加层 |
| `noxel-camera` | 俯视相机装置、死区、前瞻、像素级吸附、创伤抖动、区域 |
| `noxel-visibility` | 视锥/距离/尺寸剔除、遮挡剔除、相机遮挡淡出、LOD |
| `noxel-physics` | 刚体、SAT 精细碰撞检测、顺序冲量求解器、角色控制器、查询 |
| `noxel-world` | 确定性地形、生物群系、道路格网、城镇、区块流式加载、预制体 |
| `noxel-npc` | 人群分层、流场与 A* 寻路、转向、每日日程 |
| `noxel-debug` | 滚动统计、区段预算、叠加层、无头帧导出、图像差异比较 |
| `noxel-app` | `App`、`AppContext`、`Plugin`、固定步长帧循环 |
| `tools/noxel-gen` | 资源生成器：纹理、图集、tileset、预制体、调色板 |
| `examples/town-demo` | 可直接运行的村庄，包含玩家、NPC、物理和导出的帧 |

---

## 设计决策

引擎的各项选择都以 ADR 的形式记录在 [`docs/adr`](../adr) 中。其中塑造了其余一切的有：

| ADR | 决策 |
|---|---|
| [0001](../adr/0001-coordinate-system.md) | 右手系、`+Y` 向上、`-Z` 向前、弧度制、列主序、裁剪深度 `[0,1]`、1 单位 = 1 米 |
| [0002](../adr/0002-no-dependencies.md) | 零第三方依赖；连 PNG 编解码器也是仓库内自研 |
| [0003](../adr/0003-software-renderer-first.md) | 默认使用 CPU 光栅化器与光线追踪器；GPU 后端是一个扩展点 |
| [0004](../adr/0004-dual-renderer.md) | 一个场景、一个相机视图，在同一份数据上提供三种着色模式 |
| [0005](../adr/0005-color-management.md) | 线性 HDR 工作空间、每帧恰好一次 sRGB resolve、调色板精确往返 |
| [0006](../adr/0006-deterministic-generation.md) | 世界是 `(seed, address)` 的纯函数——任何地方都没有共享 RNG |
| [0007](../adr/0007-gpu-backend.md) | GPU 后端已完成规格设计并加了 feature 门控，但尚未实现 |
| [0008](../adr/0008-deterministic-rendering.md) | 渲染是确定性的，这正是黄金图像测试得以成立的原因 |
| [0009](../adr/0009-no-ui.md) | 没有 UI 工具包——取而代之的是调试叠加层和文档 |
| [0010](../adr/0010-testing-strategy.md) | 每条不变量都有测试；测试失败意味着要判断是代码错了还是测试错了 |

---

## 文档

英文文档位于 `docs/`。简体中文翻译位于 `docs/zh-CN/`（中文文档见 [`docs/zh-CN/`](README.md)）；两者不一致时以英文版为准。

| 文档 | 内容 |
|---|---|
| [`docs/00-overview.md`](../00-overview.md) | 引擎是什么、不是什么，以及心智模型 |
| [`docs/01-architecture.md`](../01-architecture.md) | 每个 crate、每个模块，以及一帧如何流经它们 |
| [`docs/02-getting-started.md`](../02-getting-started.md) | 从零开始构建、运行、测试和扩展 |
| [`docs/03-world-generation.md`](../03-world-generation.md) | 地形、生物群系、道路、城镇、流式加载，以及如何制作资源 |
| [`docs/04-rendering.md`](../04-rendering.md) | 三种着色模式、材质、像素风规则、阴影贴图 |
| [`docs/05-camera-and-visibility.md`](../05-camera-and-visibility.md) | 相机手感、遮挡淡出、剔除、LOD |
| [`docs/06-physics.md`](../06-physics.md) | 刚体、层、角色控制器、查询、确定性 |
| [`docs/07-npcs.md`](../07-npcs.md) | 人群分层、寻路、转向、日程，以及如何达到 1000 个 NPC |
| [`docs/08-performance.md`](../08-performance.md) | 时间花在哪里，以及如何测量它 |
| [`docs/api/`](../api) | 每个 crate 的 API 参考 |
| [`docs/contributing-for-ai.md`](../contributing-for-ai.md) | 如何在这个代码库上工作，为 AI agent 而写 |

用 `cargo doc --workspace --no-deps --open` 重新生成 API 文档。

---

## 许可证

采用 MIT 或 Apache-2.0 双许可，任选其一。
