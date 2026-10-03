> 本文是 [`docs/07-npcs.md`](../07-npcs.md) 的中文翻译。英文版是权威版本，如有歧义以英文版为准。

# NPC

`noxel-npc` 模拟一整个镇子的人：围绕移动焦点生成与回收、层次细节（LOD）分层、A\* 与流场寻路、转向，
以及每种类型各自的每日日程表。它是确定性的 —— 同一个种子产生同一个镇子 —— 并且瞄准「支持大量同时存在的 NPC」
这一需求：**1000+ 常驻代理，约四毫秒以内**。

> **实测数据。** `crates/noxel-npc/tests/crowd.rs::a_thousand_agents_stay_inside_the_frame_budget`
> 会构建一个 1000 个代理的城镇并跑 300 步。在开发机上它报告 **平均每步 1.26 ms、最差 2.16 ms、每个代理 1.26 µs**，
> 堆内存 0.9 MB —— 约为引擎 4 ms NPC 预算的三分之一，也远在测试所断言的 8 ms 平均上限之内。
> `tests/` 下有五个集成测试文件（`crowd`、`flow_fields`、`pathfinding`、`spawner`、`steering`），
> 库内另有 66 个单元测试。下文其余内容都是从源码读出来的。

## 帧

```text
maintain_population   spawn/despawn so the target population is resident
refresh_pois          the local plaza, market, tavern, work and road
assign_tiers          distance from the crowd centre decides the tier
rebuild_neighbours    one spatial hash for the whole crowd
step_agents           decide (if the tier clock says so), steer, move
resolve_paths         budgeted A* for whatever asked for a route
sync_bodies           tier-0 agents own a physics capsule
```

`update` **不会**推进物理世界：应用层在自己的固定步长循环里调用 `PhysicsWorld::step`，
人群的角色移动叠加在求解器给出的结果之上。

## 人群分层，以及为什么

一个镇子需要几百号人，而一帧只有四毫秒，所以代理的模拟级别是它到人群中心（通常就是相机焦点）距离的纯函数：

| 分层 | `TierConfig` 半径 | 决策间隔 | 转向 | 碰撞 |
|---|---|---|---|---|
| 0 `Near` | 25 m | 每步（0 s） | 完整 | 是 |
| 1 `Mid` | 60 m | 0.25 s | 完整 | 否 |
| 2 `Far` | 90 m | 0.75 s | 仅路径 | 否 |
| 3 `Frozen` | 超出 | 2.0 s | 仅路径 | 否 |

只有 0 层拥有胶囊，所以只有玩家碰得到的代理才为 `move_character` 付费；
其余所有人都按粗粒度时钟走同一条路径。`TierConfig::sanitised()` 会对半径排序并钳制（`mid >= near`、`far >= mid`，
各自 ≥ 1 m），并强制间隔有限且非负；`tier_for(distance)` 用的是严格 `<`，
所以正好等于 `near` 时是 `Mid`。`CrowdTier` 为 `Near = 0 … Frozen = 3`，
带有 `ALL`、`index()`、`from_index()`、`name()`、`Default`。

## 使用这个系统

```rust,no_run
use noxel_core::math::Vec3;
use noxel_npc::{NpcConfig, NpcContext, NpcSystem};

// `streamer` and `physics` come from the application; `center` is usually the
// camera focus and `world_time` is seconds since midnight.
let mut system = NpcSystem::new(NpcConfig::default());
let mut ctx = NpcContext::new(&streamer, &mut physics, camera, 8.0 * 3600.0, 1.0 / 60.0);
system.maintain_population(&mut ctx);
let stats = system.update(&mut ctx);
```

| `NpcSystem` 方法 | 用途 |
|---|---|
| `new(config)` / `config()` / `set_config()` | 配置会被净化；`set_config` 会重建依赖种子的状态（日程表、生成器、分层） |
| `maintain_population(&mut ctx)` | 生成/回收，使 `target_population` 常驻 |
| `update(&mut ctx) -> NpcStats` | 推进一步；返回统计 |
| `send_to(id, destination) -> bool` | 指派一个目的地并请求路径，在**下一次** `update` 时优先于人群自己的请求被处理；`false` 表示该 id 不是常驻代理，或者目的地不是有限值 |
| `crowd()` / `crowd_mut()`、`pathfinder()`、`flow_cache()`、`schedule(kind)` | 各个子系统，用于读数和测试 |
| `stats()`、`memory_bytes()` | 读数 |

`NpcContext` 为一步借用世界：`streamer`、`physics`、`center`、`world_time`（自世界启动以来的秒数）
和 `dt`。`hour_of(world_time)` 换算成小时并回绕，所以 `8.5 * 3600.0` 就是 08:30。

## `NpcConfig`

| 字段 | 默认值 | 含义 |
|---|---|---|
| `seed` | `0` | 每一次生成、日程表、目的地和路径平手裁决都由它派生 |
| `max_agents` | 4096 | 硬上限（`sanitised()` 封顶到 `1 << 20`） |
| `spawn_radius` | 70.0 m | 代理生成在中心这个半径之内 |
| `despawn_radius` | 90.0 m | 超过这个距离的代理会被退役；`sanitised()` 会把它抬到至少 `spawn_radius`，正是这个迟滞阻止了边界抖动 |
| `target_population` | 400 | `maintain_population` 保持常驻的人数（钳制到 `max_agents`） |
| `tier` | `TierConfig::default()` | 上面那张分层表 |
| `steering` | `SteeringWeights::default()` | 下面的权重 |
| `path` | `PathConfig::default()` | 下面的 A\* 调参 |
| `schedule` | `ScheduleConfig::default()` | `enabled: true`、`retarget_margin: 0.25` h |
| `agent_height` | 1.7 m | 物理胶囊高度（钳制 0.2 … 8） |
| `agent_radius` | 0.35 m | 胶囊半径与分离半径（钳制 0.05 … 4，且 ≤ 高度的一半） |
| `walk_speed` | 1.6 m/s | 钳制 0.05 … 40 |
| `run_speed` | 4.2 m/s | 逃跑时使用（钳制到 ≥ `walk_speed`，≤ 60） |
| `arrive_distance` | 0.6 m | 多近算作到达 |
| `repath_interval` | 1.5 s | 行进中两次路径刷新之间的秒数 |

`NpcSystem::new` 和 `set_config` 都会应用 `NpcConfig::sanitised()`，所以来自文件的配置不可能产生负半径、
非有限速度或为 0 的 `max_agents`。

## 代理与种类

`NpcAgent` 是带公开字段的普通数据 —— `id`、`kind`、`state`、`position`（**脚底**，米）
、`velocity`、`yaw`（弧度，`0` 朝向 `-Z`）
、`tier`、`health`、`path_cursor`、`decision_timer`、`destination`、`blocked`、`blocked_time` —— 另外还有 `new(id, kind, position)`、`is_moving()`、`speed()`（仅水平）
、`forward()`、`face()`、`distance_to()`、`to_transform()` 和 `capsule_center(height)`。`MOVING_SPEED` 是 0.05 m/s，
在 60 Hz 下不到每帧一厘米。这条记录满足 `Send + Sync + 'static`，
所以它是一个合法的 `noxel_ecs::Component`。`NpcId(pub u32)` 单调递增且**从不回收**：
回收一个代理会让该 id 在管理器的整个生命周期内退役，
所以过期的 id 永远不可能解析成另一个代理；`NpcId::INVALID` 是 `u32::MAX`。

| `NpcKind` | `speed_scale` | `max_health` | 行为 |
|---|---|---|---|
| `Villager` | 1.0 | 100 | 基准居民 |
| `Guard` | 1.05 | 140 | 更快、更耐打，黎明和黄昏在户外 |
| `Merchant` | 0.9 | 100 | 更慢，拴在摊位上 |
| `Child` | 1.1 | 60 | 短距离更快，脆弱 |
| `Animal` | 0.7 | 40 | 慢、血少、没有固定的家 |

`NpcState` 有 `Idle`、`Walking`、`Working`、`Talking`、`Sleeping`、`Fleeing` 和 `Stuck`，
带有 `ALL`、`name()` 和 `is_moving()`。一个代理如果在 `STUCK_TIME`（3.0 s）内没有报告任何进展，
就会被标记为卡住，并至多获得 `MAX_REPAIRS`（2）次重新寻路的机会；血量低于 `FLEE_HEALTH`（30）
时会以 `run_speed` 逃跑。`CrowdManager` 拥有数据，
而不拥有行为：`spawn(kind, position) -> NpcId`（满时返回 `INVALID`）
、`despawn(id)`、`agent`/`agent_mut`、`iter`/`iter_mut`/`ids`（**槽位顺序**，绝不是哈希顺序）
、`len`、`slot_count`、`slots`、`clear`。`NpcStats` 携带 `active`、`spawned_this_step`、`despawned_this_step`、`tier_counts[4]`、
路径计数器、`blocked`、`last_step_ms`、`mean_step_ms` 和 `us_per_agent`。

## 寻路：粗格上的 A\*

路径在边长为 `PathConfig::node_size` 米（**2 m**）的格子上搜索。节点是八连通的，
斜向一步只有在**两个**正交邻居都可通行时才被允许 —— 这能阻止代理从路上斜切过房屋的拐角。
有一个函数 `terrain_cost` 与流场共用，因此两者永远不会给出不一致的答案：

| 地形 | 代价 |
|---|---|
| 可行走地块，未铺装 | `1.0 × (1 + slope)` |
| 带 `TileFlags::road` 的可行走地块 | `road_cost`（0.5）`× (1 + slope)` |
| 浅于 `SHALLOW_WATER_DEPTH`（1.0 m）的水 | `water_cost`（8.0） |
| 深水、悬崖、墙（不可行走的非水域） | 不可通行 |
| 未常驻的区块 | 不可通行 |

坡度是一个**乘数**，不是闸门；`max_slope`（上升量与水平距离之比为 0.8）是在相邻节点之间单独施加的，
所以悬崖边不是廉价的捷径。
道路半价、水八倍，因此路径偏好街道网络、也会从浅滩过河，但两者都不是绝对的。**未加载的区块不可通行**，
所以人群必须待在流式加载半径之内 —— 这就是生成半径必须落在可视距离之内的原因。

| `PathConfig` | 默认值 | 净化后 |
|---|---|---|
| `max_nodes` | 4096 | `2 … 1 << 22` |
| `max_iterations` | 8192 | `2 … 1 << 24` |
| `node_size` | 2.0 m | `0.25 … 64` |
| `max_slope` | 0.8 | `> 0 … 10` |
| `water_cost` | 8.0 | 有限、非负 |
| `road_cost` | 0.5 | 有限、非负 |
| `heuristic_weight` | 1.1 | `0.1 … 4.0` |

`heuristic_weight` 大于 1.0 是刻意不可采纳的：用一点最优性换大量速度，
在村庄尺度上根本看不出来。`Path` 携带 `waypoints`、`cost` 和 `complete`（当搜索停在最近的可达节点时为 `false`）
，
并提供 `empty()`、`direct(from, to)`、`len()`、`length()`、`next_waypoint(cursor)`、`advance(cursor, position, radius)` 和 `simplify(tolerance)`。`Pathfinder::find_path(streamer, from, to)` 就是搜索；`stats()`、`cache_len()` 和 `clear_cache()` 是读数。
步预算限定最坏帧：`MAX_PATHS_PER_STEP`（24 次搜索）、`MAX_NODES_PER_STEP`（16 384 次扩展）
、`PATH_CACHE_CAPACITY`（256 个结果）、`GOAL_SNAP_RINGS`（4，把落在墙内的目标往外推）
和 `MAX_CLAMP_STEPS`（128）。**确定性：** 前沿是一个二叉堆，先按 `f` 的 `total_cmp` 排序，再按节点坐标排序，
所以平手时弹出的顺序是固定的，
而不是哈希顺序 —— `the_same_request_twice_is_identical` 和 `two_pathfinders_agree_bit_for_bit` 对此做了断言，`tests/pathfinding.rs` 还覆盖了深水、
未加载即不可通行的行为以及缓存。

## 流场：一次搜索，多个代理

两百个村民奔向同一个摊位，不该跑两百次搜索。`FlowField` 把答案算一次：在目标周围开一张粗网格，
每个格子存放最能缩短与目标距离的方向。当目标落在流式加载区域之外、墙内，
或者深到无法涉水的水里时，`FlowField::build(streamer, goal, extent, cell)` 返回 `None`；`direction_at(p)`、`cost_at(p)` 和 `contains(p)` 是读取接口。
默认值：`FLOW_CELL` 2.0 m、`FLOW_EXTENT` 60.0 m、`FLOW_CACHE_CAPACITY` 24 个流场、`MAX_FLOW_EXTENT` 512 m、`MAX_FLOW_SIDE` 96 个格子。
缓存以**量化后的目标**为键，所以相距几厘米的目的地共用一个流场；淘汰是严格的 LRU，
其顺序存在一个有序索引里而不是靠遍历哈希表，因此一次繁忙的帧之后哪些流场能留下来，
是所发请求的确定性函数。`find` 从不重建；它才是逐步循环调用的那个函数。

当许多代理共享一个会静止一阵子的目的地时，流场胜过 A\* —— 每日日程表产生的正是这种情况。单个代理、
一个独一无二的目的地，或者一小群人时，A\* 胜出。广场上的人流涌进酒馆而 `paths_computed` 保持平稳，
就是缓存正在起作用的信号。

## 转向

`Steering::force` 把 Reynolds 行为求和成一个加速度；系统会在第二次钳制之前加上两个依赖世界的项。

| 项 | 作用 | 默认权重 |
|---|---|---|
| seek | 朝期望方向加速 | 1.0 |
| separation | 推开过近的邻居 | 1.6 |
| alignment | 与邻居的平均朝向对齐 | 0.15 |
| cohesion | 向邻居的中心靠拢 | 0.1 |
| obstacle | 避开探针打到的东西 | 2.0 |
| road bias | 拉向最近的道路 | 0.25 |

`max_force` 是 12.0 m/s²，`neighbour_radius` 是 2.0 m，`separation_radius` 是 0.9 m。分离力随人群增大；
钳制不会 —— 正是这一点阻止了门口二十个代理把谁弹飞出去。`SteeringWeights::sanitised()` 会把非有限的权重替换成默认值。`Steering::build_neighbours(agents, cell)` 每步构建一张空间哈希，
而不是逐一测试每一对。`Steering::avoid(agent, desired, streamer, probe)` 在 `AVOID_ANGLE`（0.698 rad ≈ 40°）
上向左、中、右三个方向以 `DEFAULT_PROBE`（2.0 m）探测，用的是 streamer 的可行走性，
并拒绝任何爬升超过 `AVOID_MAX_RISE`（0.8 m）的侧移 —— 这套避让遵守与控制器相同的坡度限制。
转向产生的是**期望位移**，而不是瞬移；0 层把它交给 `move_character`。

## 每日日程表

一天是一串排好序的 `ScheduleEntry` 边界，恰好一次覆盖 `[0, 24)` —— 这条不变式让查找变成二分搜索，
也让 `to_text()` 读起来像一张时刻表（`00:00 sleep home`、`06:30 eat tavern`、`07:00 commute road`、`08:00 work work`、`22:00 sleep home`）
。`Activity`（`name()`、`place()`）是代理在做什么，`ActivityPlace`（`name()`）
是在哪里。`DailySchedule::for_kind(kind, seed)` 让一天成为 **`(kind, seed)` 的纯函数** —— 没有逐代理存储 —— 并提供 `activity_at(hour)`、`current(hour)`、`next_boundary(hour)`（跨午夜回绕）
、`validate()`、`to_text()`、`total_hours()`、`len()` 和 `memory_bytes()`。`ScheduleConfig` 是 `enabled`（设为 false 会让所有人在中心附近游荡，
这正是转向调试视图想要的）加上 `retarget_margin`（默认 0.25 h）：代理会提前这么久朝下一个活动的地点出发，
所以店主在七点五十就走向店铺，而不是站在家里。边界由种子派生，所以两个村子会保持略微不同的作息，
同时与作者设定的时间相差不到一小时。
系统把日程表解析为每个人群五个**局部兴趣点** —— `plaza`、`market`、`tavern`、`work` 以及最近的 `road` —— 当中心移动超过 `POI_REFRESH_DISTANCE`（10 m）
时刷新。要保存的是**种子**，不是日程表。

## 生成器

| 条目 | 用途 |
|---|---|
| `NpcSpawner::new(config)`、`config()`、`set_config()` | |
| `choose_spawn(streamer, center, index) -> Option<(NpcKind, Vec3)>` | 在 `spawn_radius` 之内给出一个种类和一个地面位置，按 `(seed, index)` 确定 |
| `kind_for(streamer, position, index) -> NpcKind` | 一个地点所暗示的种类 |
| `classify(streamer, position) -> SpawnKind` | `Interior`、`Road`、`Town` 或 `Wilderness` |

生成点是代理能站的地方（常驻区块、可行走地块、水面之上、不在建筑或道具内部），并且**地点决定种类**：
建筑包含该点时为 `Interior`，然后是 `Road`（`is_road_at`），再是 `Town`，
否则为 `Wilderness` —— 未常驻的区块报告 `Wilderness`，也就是「主张最少的那种答案」
。`choose_spawn` 按固定的 `PLACE_WEIGHTS` 抽取一个偏好地点并重试，退回到任何可用点，最后退回到黄金角螺旋，
全都来自 `RngStream::indexed(seed, "npc/spawn", index)`，所以重新填充的镇子会在同样的地方得到同样的人。

## 与物理和场景的集成

| 层 | 拥有 |
|---|---|
| `noxel-npc` | 代理状态、分层、寻路、转向、日程表、0 层胶囊 |
| 游戏 | 每个代理一个 `Scene` 实例，按种类给材质，按分层给标志 |
| `noxel-physics` | 求解器；应用层自己调用 `step` |

为每个代理保留一个实例，以 `agent.id.index()` 为键，第一次见到时创建、id 消失时移除（否则玩家一走，
人群就会无界增长）。给每个代理 `InstanceFlags::character()`，在近层以下把 `occluder = false`，
否则 400 个代理每帧都会往相机遮挡的射线束里塞 400 个盒子。`capsule_center(height)` 和 `to_transform()` 是把代理镜像出去所需的两个调用。

## 一个完整的「镇里 1 000 个 NPC」示例

**实测：平均每步 1.26 ms、每个代理 1.26 µs、最差 2.16 ms、0.9 MB**，条件是 1000 个代理
在 60 Hz 步长下跑 300 步。这正是整个 crate 围绕它设计的数字 —— 在引擎 4 ms 的 NPC 预算之内；
套用「在 4 ms 预算下，至多 `4000 / us_per_agent` 个代理都是免费的」这条规则，
大约可以容纳 3000 个代理。

测试脚本是 `crates/noxel-npc/tests/crowd.rs`，其结构大致是：

```rust,no_run
use noxel_npc::{NpcConfig, NpcContext, NpcSystem};

let mut system = NpcSystem::new(NpcConfig {
    target_population: 1_000,
    spawn_radius: 60.0,
    despawn_radius: 80.0,
    ..NpcConfig::default()
});
// ... build a streamer and a physics world, then step ~600 times ...
// let stats = system.update(&mut ctx);
// assert_eq!(stats.active, 1_000);
// assert_eq!(stats.tier_counts.iter().sum::<usize>(), 1_000);
// assert!(stats.us_per_agent * stats.active as f32 <= 8.0,
//         "1000 agents cost {:.2} ms", stats.last_step_ms);
```

先断言**人口和分层分布**，再用 `us_per_agent` 表达一个宽松的上界，因为预算说的就是每代理成本。
一个断言「1 000 个代理 6.4 ms」的测试，记录下来的只是某一台笔记本（`docs/08-performance.md`）。
不用计时就能暴露回归的计数器：`paths_computed` 与 `path_nodes_expanded`（流场热起来之后就持平了）
、`flow_fields_cached`、`blocked`，以及 `tier_counts` 的分布。

## 性能旋钮，按收益从大到小排序

1. **分层半径与间隔** —— `Near` → `Mid` 把决策频率从每步一次变成每秒四次，而离开 0 层之后代理就不再碰撞了。
2. **用流场代替逐代理 A\*** —— 按目的地搜索而不是按代理搜索；盯住 `flow_fields_cached` 和路径计数器。
3. **每步路径预算** —— `MAX_PATHS_PER_STEP`、`MAX_NODES_PER_STEP` 和 `PATH_CACHE_CAPACITY` 给最坏帧封顶，`repath_interval`（1.5 s）则封住一个代理多久才会再问一次。
4. **`node_size`** —— 2 m 的格子只有 1 m 格子四分之一的节点数，而且 `Path::simplify` 反正会把结果抹平。
5. **`target_population` 与 `spawn_radius`** —— 人口会把一切放大；半径决定其中有多少在屏幕上。
6. **每个分层的 `collides` 以及在其之下 `occluder = false`** —— 为碰不到的代理准备胶囊和遮挡体积，收益很小。
7. **`Steering::build_neighbours` 的格子尺寸** —— 每帧 O(n) 而不是 O(n²)。
8. **场景实例的频繁增删** —— 池化实例并切换 `Instance::visible`，而不是每帧销毁再重建。

`NpcSystem` 会把 `dt` 钳制到 `MAX_DT`（0.1 s）；喂给它真实帧间隔而不是固定步长，会让上面的一切都变得不确定。

## 常见错误

- **把帧间隔当作 `dt` 传进去。** 人群是按固定步长调校的，并在 `MAX_DT` 处钳制；可变步长会破坏确定性。
- **在流式加载的世界之外生成。** 未常驻的区块不可通行，所以这样的代理永远无法寻路到任何地方：它站着不动，看起来像坏了。
- **手动改一个分层半径。** 跑一遍 `TierConfig::sanitised()`（系统会自己跑）；否则一个无序的配置会产生永远离不开 3 层的代理。
- **把 `agent.position` 当作胶囊中心。** 它是**脚底**位置；物理请用 `capsule_center(height)`，渲染实例请用 `to_transform()`。
- **在人群更新里调用 `PhysicsWorld::step`**，或者指望 `send_to` 立刻寻路（它会在下一次 `update` 中、在步预算之内被处理）。
- **忘了深水是不可通行，而不是代价高。** 水代价只在 `SHALLOW_WATER_DEPTH` 以下生效；更深的都不可通行。
- **以为 A\* 是最优的。** `heuristic_weight` 默认是 1.1；改为断言路径*能到达目标且不切角*。
- **逐代理存储日程表，或者写出跳过某个小时的日程。** `[0, 24)` 必须恰好被覆盖一次；保存种子，而不是日程表。
- **清空人群后还指望 id 依然有效**，或者用一段耗时而不是断言人口、分层分布和计数器来衡量人群。
