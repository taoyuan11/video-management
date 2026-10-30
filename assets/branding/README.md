# 帧库 · 叠帧

三张叠放的视频帧表示收集与整理，前景的播放符号表示随时回看。标志沿用应用的 Fluent 蓝色，从浅青蓝过渡到 `#005FB8` 附近的强调蓝；圆角、轻微错位和边缘高光体现层次。无需文字即可在小尺寸下识别，透明画布适用于浅色与深色背景，卡片及播放符号保持实色。

- `frame-library.svg`：可编辑的 512 × 512 矢量原稿，唯一设计源。
- `frame-library.png`：1024 × 1024 透明 PNG，用于展示或其他品牌素材。
- `frame-library-256.png`：256 × 256 透明 PNG，编译进窗口和侧栏。
- `frame-library.ico`：16、20、24、32、40、48、64、96、128、256 px，32 位 RGBA，适配 Windows 11 常见缩放比例。
- `windows.rc`：资源 ID 1，编译进主程序和更新器，供资源管理器、快捷方式及任务栏读取。

所有运行资源均嵌入 EXE，分发时无需附带 SVG、PNG 或 ICO。主窗口与待处理窗口同时设置窗口小图标及原生 `ICON_BIG`（任务栏／Alt+Tab）。品牌图标不随主题改色，也不随 Acrylic 或不透明回退改变透明度。

## 修改与导出

先编辑 SVG，再用 Node.js 与 `sharp` 执行 `scripts/generate-icons.cjs`。此步骤只在修改图形时需要，普通 Cargo 构建直接使用仓库内的成品，不依赖 Node.js。

```powershell
# 在项目根目录安装独立的导出工具，不修改 Rust 构建环境。
npm install --prefix .qa/icon-tools --no-package-lock --no-save sharp
node scripts/generate-icons.cjs ../.qa/icon-tools/node_modules/sharp
cargo build --locked --bin video-management
```

此图形为直接绘制的原创矢量标志，未调用图像生成 API。设计规格：Windows 11 Fluent 风格、三张叠放的视频帧、白色圆角播放符号、蓝色渐变、透明背景、无文字，兼顾 16 px 标题栏与高 DPI 图标。
