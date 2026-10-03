> 本文是 [`docs/contributing-for-ai.md`](../contributing-for-ai.md) 的中文翻译。英文版是权威版本，如有歧义以英文版为准。

> **先读[实战笔记](实战笔记.md)。** 那是这个引擎上做一个真实游戏所付出的全部代价：
> 不产生任何报错的坑、管用的技巧、以及本可以抓出其中大部分的验证习惯。
> 本文是规则，那一篇是经验。

# 以 AI agent 的身份贡献

本仓库主要由 AI agent 并行地、逐个 crate 地编写。这种工作流有一个主要失败模式：**能编译、看起来合理、但微妙地错误的代码**。本文档是避免它的操作手册。

如果你只读一节，请读「判断错的是代码还是测试」以及末尾的陷阱列表。

## crate 的 DAG

依赖**单向**向下。一个 crate 可以依赖此列表中位于它上方的任何东西，而不能依赖下方的任何东西，且不存在环。

| 层 | crate | 依赖 |
|---|---|---|
| 0 | `noxel-core` | — |
| 1 | `noxel-ecs` | core |
| 1 | `noxel-asset` | core |
| 1 | `noxel-physics` | core |
| 2 | `noxel-render` | core、asset |
| 2 | `noxel-world` | core、asset |
| 3 | `noxel-camera` | core、render |
| 3 | `noxel-debug` | core、asset、render |
| 4 | `noxel-visibility` | core、ecs、render、camera |
| 5 | `noxel-npc` | core、ecs、physics、world |
| 6 | `noxel-app` | 以上全部 |
| — | `tools/noxel-gen` | core、asset |
| — | `examples/town-demo` | 以上全部 |

```text
core ─┬─ ecs ─────────────────────────────┐
      ├─ asset ─┬─ render ─┬─ camera ─────┤
      │         │          ├─ debug       │
      │         └─ world ──┼─ npc ────────┤
      └─ physics ──────────┘              └── app
```

从 DAG 中得出的规则：

- **下面的层不得知道上面的层。** `noxel-physics` 不能知道 `Scene` 是什么；`noxel-world` 不能知道相机是什么。当下层看起来需要上层的某样东西时，答案是下层 crate 里的一个数据类型（一个 `ColliderShape`、一个 `Aabb`、一个 `&[RoadSegment]`）或一个回调。
- **`noxel-core` 是所有人共享的唯一 crate。** 你放进那里的任何东西，都由其他每个 crate 买单，因此它必须是通用的、并被重度测试。
- **新增一条边是一项设计决策**，不是图方便。如果你需要一个新的依赖，先检查已有的下层类型是否已经携带了这些数据；如果没有，这个类型多半应该放在 `noxel-core`。

去验证这张图，而不是信任这张表：

```bash
grep -A12 '^\[dependencies\]' crates/*/Cargo.toml
```

## 硬性规则

每个 crate 的 `lib.rs` 都以这些开头：

```rust
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]
```

| 规则 | 对你的后果 |
|---|---|
| `forbid(unsafe_code)` | 没有逃生舱。切片索引、类型转换与不变量都必须用安全 Rust 表达。在真正别扭的地方，crate 会记录其变通做法（见 `PaletteFile::to_palette`，它故意泄漏条目名，并以调色板大小为界） |
| `deny(missing_docs)` | **每个公共条目都需要文档注释**，并且 `cargo doc` 失败就是构建失败。模块级 `//!` 文档应当解释该模块存在的*原因*，而不是复述它的名字 |
| `warn(clippy::all)` | CI 把 clippy 警告当作错误；见下面的命令 |
| 零第三方依赖 | `[workspace.dependencies]` 只列出 Noxel crate 与 `std`。没有 `serde`、没有 `glam`、没有 `rand`、没有 `rayon` —— 见 `docs/adr/0002-no-dependencies.md`。GPU 后端被*规定*为 feature-gated（`docs/adr/0007-gpu-backend.md`），但目前还不存在 `[features]` 表 |
| edition 2024，`rust-version` 1.85 | `rust-toolchain.toml` 把工具链钉在 `stable`，带 `rustfmt` 与 `clippy`。Edition 2024 让 `gen` 成为保留关键字（见陷阱一节） |
| `rustfmt.toml` | `max_width = 100`、`hard_tabs = false`、`tab_spaces = 4`、Unix 换行、`reorder_imports`、`use_small_heuristics = "Default"` |
| 无 UI 工具包 | `docs/adr/0009-no-ui.md`：没有菜单、没有布局引擎、没有编辑器。`noxel-render::overlay` 画文本与线条；它不做输入路由，也不做任何布局 |
| 确定性 | 任何渲染器、采样、生成或 NPC 代码都不得读取时钟、线程 id 或全局 RNG —— `docs/adr/0008-deterministic-rendering.md` 与 `docs/adr/0006-deterministic-generation.md` |

当你推理性能时，构建 profile 很重要：

| Profile | 设置 |
|---|---|
| `dev` | `opt-level = 1`、`debug = true`、`overflow-checks = true` —— **不是**性能测量环境 |
| `release` | `opt-level = 3`、`lto = "fat"`、`codegen-units = 1`、`panic = "unwind"`，符号被剥离 |
| `bench` | 继承 release，保留调试信息与符号 |

## 命令

`cargo` 安装在 `~/.cargo/bin` 下，而在**这台机器上全新的非登录 shell 里它不在 `PATH` 上**。下面每条命令都以这行开头：

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

然后，在仓库根目录：

```bash
cargo fmt                                       # format everything
cargo fmt --check                               # what CI would reject
cargo clippy --all-targets -- -D warnings       # lints, warnings are errors
cargo test                                      # the whole suite
cargo test  -p noxel-world --lib                # one crate, one target
cargo test  --workspace --exclude town-demo --no-fail-fast   # keep going past a failure
cargo doc   --workspace --no-deps --open        # the API reference
cargo check --workspace --all-targets           # the fastest "does it build" signal
```

省时间的注意事项：

- `cargo test` 会在第一个失败的 *crate* 处停下。想看全貌时用 `--no-fail-fast`。
- `cargo test -- --nocapture` 会显示测试里的 `println!` 输出。
- dev profile 有 `overflow-checks = true`，这是刻意的：算术溢出会在测试中 panic，而不是悄悄回绕。
- 只在 `--release` 下测量性能。在 `dev` 下，`opt-level = 1` 与真实数字大约差一个数量级，而且它是一个不同的数，而不是一个更小的数。

`.github/workflows/ci.yml` 会在 push 和 pull request 上运行同一组步骤：格式检查、构建、带 `-D warnings` 的 clippy、测试、文档、`noxel-gen verify` 以及一次无头 demo 渲染。`scripts/check.sh` 在本地跑同一套关卡，`--fast` 会跳过 release 构建和 demo。改了这里的命令就要同步改工作流，否则两者会漂移。

## 来自测试策略的约定

`docs/adr/0010-testing-strategy.md` 是权威文档。其中会改变你写代码方式的部分：

1. **每个 crate 都在自己的模块内携带单元测试**，每一条非显然的不变量都有一个点名的测试。
2. **测试名是句子**：`parallel_and_sequential_rendering_agree`，而不是
   `test_render_2`。
3. **对测试从几何计算出来的值做断言**，绝不使用从一次失败运行中抄来的数字。`nearest_finds_the_closest` 断言距离落在 `8.5..9.0`，是因为测试能证明最近点在 8.7 之外。
4. **依赖某种顺序的测试断言该顺序**，而不是绝对值。
5. **除了显式测量墙上时钟的测试之外，没有测试依赖墙上时钟**；而那些测试断言的是顺序不变量，绝不是某个时长。
6. **每一条与 `unsafe` 相邻的假设都是一个测试**：索引算术、位集迭代、世代句柄复用、`swap_remove` 重指向。
7. **每一个带有看似随机输入的系统，确定性都有一个显式测试**。典型形态：`two_generation_orders_agree`、
   `determinism_is_bit_exact`、`parallel_and_sequential_rendering_agree`。

四个类别，价值递增：契约测试（API 形状与默认值）、不变量测试（必须成立的事）、派生数值测试，以及黄金图像测试（用 `noxel-debug::dump::compare` 对照一张已提交的 PNG）。

## 判断错的是代码还是测试

**当测试失败时，判断错的是哪个：代码还是测试。两者都会发生。**
这是仓库里最重要的判断，搞反了就是测试套件不再成为证据的方式。

按这个顺序过一遍：

1. **读失败本身，而不是测试名。** 断言消息与两侧的值告诉你代码相信什么。
2. **问测试在断言什么，以及那个主张从何而来。** 一个断言已文档化不变量的测试（「未受光的精灵往返回到创作的字节」「每个程序化网格都朝外绕序」）是在陈述契约。那就是代码错了。
3. **问期望值是推导出来的还是猜出来的。** 如果期望的数字是有人从上次运行里粘过来的字面量，那这个测试测量的是巧合。重新推导它，并带着推导过程重写测试，把推导写进注释。
4. **检查单位。** 数量惊人的「bug」其实是 `radians` 与 `degrees`、`[0, 1]` 与 `[-1, 1]` 深度，或某个偏航角符号。
5. **检查测试是否在断言超出代码承诺的东西。** 对一个平滑值要求精确浮点相等的测试是错的；对一个*经由调色板的往返*要求精确相等的测试是对的。
6. **如果你改了测试，请在注释里解释原因。** 否则下一个 agent 会假定测试是对的，再把它「改回去」。

### 测试是对的的实例

这些是这套测试在开发期间抓到的真实回归。每一个都是你应该认得出来的形态。

| bug | 症状 | 测试断言了什么 |
|---|---|---|
| 环境半球混合反向 | 朝上的表面比朝下的更暗 | `ambient_hemisphere_blend`：朝上的法线得到的天空光多于地面光 |
| `damp_factor(0)` 冻结而不是吸附 | 相机永远到不了位，而 `dt == 0` 的一帧会把它传送过去 | `smoothing_zero_snaps`（相机），加上 `damp_factor` 自身 doctest 中的复合性质 |
| 没有深度测试的光栅化器 | 远处的表面画在了近处表面之上 | `depth_buffer_keeps_the_nearer_surface`：两个平面，近的必须胜出 |
| 把非 ASCII 截断成单字节的字体 | `text_width` 与笔位在多字节字符上不一致，于是文本重叠 | `font_missing_glyph_is_none` 与 `text_width_math`：宽度是 `chars()` 的函数，而不是字节的函数 |
| 重复 blit 的泛光通道 | 辉光大约比预期亮一倍 | `bloom_is_energy_positive_but_bounded`：加上泛光不得把图像乘起来 |
| 共享三角形边被着色两次 | 细分地板接缝处出现深色拼布，透明四边形上出现深色对角线 | 填充规则本身，加上 `parallel_and_sequential_rendering_agree`（两条路径不得在接缝处产生分歧） |
| UV 球的极点瑕疵 | 极点环塌缩成一个点，于是绕序测试看到面积为零、法线无意义的三角形 | `sphere_faces_wind_outwards` |

最后一行是有用的反例：那里错的是*测试*，不是代码。`Mesh::sphere` 确实在两个极点产生退化三角形（UV 球的顶环与底环塌缩成一点），而这无害——光栅化器在它们到达像素之前通过 `ScreenTriangle::is_degenerate`（`area.abs() < 1e-7`）丢弃它们。修法是在绕序测试里跳过零面积三角形并写明原因，而不是去「修」网格。如果你看到退化几何上的失败测试，先检查这种退化是否是构造本身固有的。

## 陷阱

下面每一条都在这个代码库里真实浪费过别人的时间。

| 陷阱 | 细节 |
|---|---|
| **列主序索引** | `Mat4::get(row, col)` 索引的是 `cols[col][row]`，与字段顺序相反。它是 `math/mat.rs` 中最常见的困惑来源，因此用显式的 `match` 实现以保持 `const`。当一个变换看起来被转置了，先查这里 |
| **`-Z` 是前方** | 右手系、`+Y` 为上、`-Z` 为前方，偏航角 `0` 朝向 `-Z`（`Vec3::from_yaw(0) == (0, 0, -1)`），正俯仰角向下看，`PI/2` 是垂直向下。`Quat::to_yaw` 的符号错误已经发生过两次；`quat_yaw_round_trips` 就是为此存在的（`docs/adr/0001-coordinate-system.md`） |
| **`ChunkPos::y` 是 Z** | 区块坐标是 `{ x, y }`，其中 `y` 是世界 **Z** 轴。`chunk.pos.y` 不是高度，而 `Aabb` 上的 `size.x`/`size.z` 是水平范围，`size.y` 才是向上 |
| **`gen` 在 edition 2024 中是保留关键字** | 生成器模块声明为 `pub mod r#gen;`，并通过 `noxel_world::r#gen` 访问。crate 根会再导出它的公共条目，因此调用方通常从不需要打出原始标识符 |
| **`Scene::Instance::bounds` 是私有的** | `Instance` 把 `bounds()` 暴露为方法；字段不能被赋值。先写 `instance.transform`，再调用 `Scene::update_bounds(handle)` 或 `update_all_bounds()`，否则渲染器与剔除器会继续使用旧的包围盒 |
| **`Light` 没有 builder 方法** | `Light` 是带公共字段的枚举。没有 `with_direction`/`with_intensity`；请构造 `Light::Directional { .. }` 或对变体做匹配。`Ambient` 与 `Fog` 同样如此（不过 `Fog`、`Ambient::night` 与 `Ambient::interior` 是预设） |
| **`AlphaMode::Additive` 不做加法** | 光栅片元路径把它当作 `Blend` 一样做 source-over 合成，alpha 钳到 1.0。`Framebuffer::add` 存在，但只被调试图块叠加层使用。不要在没查过这一点的情况下用 `Additive` 做辉光 |
| **`AppConfig::mode` 不设置色调曲线** | `App::resolve()` 调用 `self.config.render.resolve()`，而 `AppConfig::with_mode` 只写 `config.mode`。一个用默认 `render` 设置的光线追踪 app 会以 `ToneMap::None` 解析 |
| **区段预算是可选的** | `DebugSystem::record_section` 只在 `DebugConfig::record_sections` 为 true 时记录，而 `DebugConfig::disabled()` 把它设为 false。「消失」的测量值通常是这个原因 |
| **一半的 `FrameSample` 字段没有被填** | `release`、`visibility_ms`、`stream_ms`、`npc_ms` 与 `physics_ms` 存在，但引擎里没有任何东西写它们；游戏通过名为 `physics`、`visibility`、`stream` 与 `npc` 的预算来填 |
| **`--all-targets` 才是诚实的检查** | `cargo check --workspace` 只编译库，跳过所有测试与示例目标，因此它可以在 `cargo test` 构建失败时依然通过。用 `cargo clippy --workspace --all-targets -- -D warnings`，这也是 CI 运行的命令。 |
| **`tools/noxel-gen` 写出字节稳定的输出** | JSON 保持作者的键顺序，图集打包先按高度再按名字排序，PNG 编码是固定算法。重排键或条目的改动会产生毫无理由的巨大 diff |

## 完成的定义

在你把一项改动报告为完成之前：

1. `cargo fmt` 与 `cargo clippy --all-targets -- -D warnings` 是干净的。
2. `cargo test` 通过，或者每一处失败都是你能用一句话解释的。
3. 新的公共条目有文档注释，新的不变量有为之点名的测试。
4. 新代码遵守 DAG：没有无理由新增的依赖，没有向上的引用。
5. 你写进报告的任何数字都能由你运行过的某条命令复现，并且你要说明是哪条命令。像「这更快了」这样没有在同一配置下做前后测量的主张，不算结果。
