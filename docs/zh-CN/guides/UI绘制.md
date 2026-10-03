> 本文是 [`docs/guides/ui.md`](../../guides/ui.md) 的中文翻译。英文版是权威版本，如有歧义以英文版为准。

# 在没有 UI 工具包的情况下绘制 UI

Noxel 不提供 UI 系统（`adr/0009-no-ui.md`），但它确实提供构建 UI 所需的原语，而且它们是精确的：叠加层以你所请求 sRGB 颜色的线性值绘入**线性**帧缓冲，因此用 `#FF0000` 画出的像素解析回来恰好是 `#FF0000`。

你可以在三个层次上工作。

---

## 1. 文本与盒子——`noxel_debug::overlay`

```rust
use noxel_debug::overlay::{DebugPanel, PanelSlot};
use noxel_core::math::Color8;

let panel = DebugPanel::new(
    PanelSlot::BottomRight,
    vec!["HP 12/20".to_string(), "GOLD 143".to_string()],
);
panel.draw(&overlay, framebuffer, 0, 1.0);
```

`PanelSlot` 锚定到一个角；`DebugPanel::draw` 返回下一个空闲的 y，因此面板会堆叠。如果你需要自己布局，`DebugPanel::size` 给出像素尺寸。

---

## 2. 原始图元——`noxel_render::overlay::Overlay`

| 调用 | 绘制 |
|---|---|
| `text(fb, x, y, text, color)` | 用内置 3x5 字体绘制一个字符串 |
| `text_scaled(fb, x, y, text, color, scale)` | 同样的内容，按整数倍缩放 |
| `screen_rect(fb, x, y, w, h, color, filled)` | 一个矩形 |
| `screen_line(fb, x0, y0, x1, y1, color)` | 一条线 |
| `panel(fb, x, y, &lines, fg, bg)` | 一个带边框的填充面板 |
| `crosshair(fb, x, y, size, color)` | 一个十字 |
| `line/aabb/ray/cross(fb, camera, ...)` | 投影后的 3D 图元 |

`Overlay::set_depth_test(false)` 让 3D 图元穿透世界绘制，这是调试视图想要的，而世界空间血条*不*想要。

字体是覆盖大写 ASCII、数字与常见标点的 3x5 位图。它刻意不足以支撑对话：需要真实文本的游戏自带字体图集（`noxel-asset::atlas` 可以打包一个）并绘制四边形。

---

## 3. 真正的 UI——你自己的四边形

对于带边框与布局的菜单，请构建网格，并用正交相机通过场景绘制它们。一个 2D UI 层就是：

```rust
use noxel_camera::TopDownCamera;
use noxel_camera::ProjectionMode;
use noxel_render::framebuffer::Framebuffer;
use noxel_render::scene::Scene;

/// A second camera looking at the UI plane.
fn ui_camera(height: f32) -> TopDownCamera {
    let mut camera = TopDownCamera::new(ProjectionMode::Orthographic { height });
    camera.set_smoothing(0.0);
    camera
}

fn draw_ui(app: &mut noxel_app::App, framebuffer: &mut Framebuffer) {
    // 1. Render the UI scene with an orthographic camera whose view height is
    //    the internal height in pixels, so one world unit is one pixel.
    // 2. Or, for a HUD, draw directly with `Overlay` after `app.render()`.
    let _ = (app, framebuffer);
}
```

对于一个永不移动的 HUD，叠加层路径更简单也更快。对于一个带九宫格面板、动画转场与文本框的菜单，第二个带正交相机的场景才是对的形状：它复用渲染器、材质系统与纹理图集，代价只是多一次 `Renderer::render` 调用，画进同一个帧缓冲并清空深度缓冲。

---

## 让 UI 随窗口缩放

内部分辨率是固定的，因此 UI 以内部像素为单位创作。一条血条在 320x180 下是 24 像素宽，并在任何窗口尺寸下都保持 24 内部像素宽——这正是像素艺术游戏想要的。

如果游戏有 UI 缩放设置，请缩放*创作*时的数字，而不是缩放投影，否则文本会变成模糊的放大结果。

---

## 常见错误

**「我的文本看不见。」**
帧缓冲是线性的且从零开始；暗色画在暗背景上就是看不见。先画一个填充背景（`screen_rect(..., true)`）。

**「颜色不对。」**
你在期望 sRGB `Color8` 的地方传了线性颜色，或者反过来。`Color8` 是 sRGB 字节，`Color` 是线性辐射度；`Color8::to_linear` 与 `Color::to_srgb8` 负责转换。

**「我的叠加层画在世界下面了。」**
叠加层在场景*之后*写入帧缓冲，因此它按构造就在最上面。如果它看起来被埋住了，是深度测试在拒绝它：调用 `set_depth_test(false)`。

**「HUD 随窗口缩放，但游戏画面不缩放。」**
你在用窗口的尺寸绘制，而不是内部尺寸。请用 `framebuffer.width()` 与 `framebuffer.height()`。
