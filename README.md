# MiBand Pulse Overlay

> A desktop overlay that shows your Mi Band's real-time heart rate on top of everything, with an optional built-in HTTP server you can drop into OBS as a browser source.

把小米手环的实时心率显示为桌面悬浮窗（核心功能），同时内置一个可开关的本地 HTTP 服务（副功能），可作为 OBS 浏览器源。

> 本项目基于 [Tnze/miband-heart-rate](https://github.com/Tnze/miband-heart-rate) 二次开发。
> 原作者的代码继续遵循 MIT 协议许可，个人新增的代码采用
> [GNU Affero General Public License v3.0 (AGPL-3.0)](https://www.gnu.org/licenses/agpl-3.0.html) 协议开源。
> 详见根目录下的 [LICENSE](LICENSE)、[LICENSE-MIT](LICENSE-MIT) 和 [LICENSE-AGPL-3.0](LICENSE-AGPL-3.0)。

## 主要特性

### 桌面悬浮窗（核心）

- 透明、无边框、始终置顶，从任务栏隐藏
- Windows 下使用 Win32 子类化 + DWM API 彻底剥离窗口边框
- 心形脉冲动画 + 大字号心率数字 + 最高 / 平均统计
- 鼠标点击穿透（悬浮窗不挡操作）
- 窗口位置自动记忆（下次启动恢复）
- 无信号 >5s 标记为断开，>10s 自动隐藏（可关闭）
- 5 秒一次重新置顶，防止被其他置顶窗口遮住
- 信号状态颜色：蓝色（已连接 · 接触良好）/ 橙色（未接触皮肤）/ 灰色（断开）

### 系统托盘

- 固定位置 / 打开 OBS Server / 打开设置面板 / 自动隐藏
- 重置窗口位置 / 打开数据目录 / 显示窗口 / 退出

### 设置面板

- 透明度、字体大小、颜色等
- 持久化到用户数据目录

### OBS 广播（可选副功能）

- 托盘菜单里"打开 / 关闭 OBS Server"启停
- 默认监听 `http://127.0.0.1:3030`
- `/heartrate` 返回当前心率 JSON
- `/` 返回悬浮窗页面（与桌面悬浮窗同样的内容）

## 数据目录

Tauri 根据 `identifier` 决定用户数据目录，存放 `settings.json` 和 `window-pos.json`：

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\com.miband.pulse.overlay\` |
| macOS | `~/Library/Application Support/com.miband.pulse.overlay/` |
| Linux | `~/.local/share/com.miband.pulse.overlay/` |

托盘菜单"打开数据目录"可直接定位。

## 支持的平台

使用 [`bluest`](https://crates.io/crates/bluest) 库，支持：

- Windows 10 / 11
- macOS / iOS
- Linux

## 支持的手环

小米手环 10 Pro / MiBand 10 Pro。

需要在小米运动健康 App 的"心率广播"中开启广播功能。

## 开发

```bash
cargo tauri dev
```

## Screenshot

![Screenshot](doc/screenshot.png)

## License

本项目采用双协议授权：

- 原作者 [Tnze](https://github.com/Tnze) 编写的代码遵循 **MIT License**，
  详见 [LICENSE-MIT](LICENSE-MIT)。
- 本仓库新增 / 修改的代码遵循 **GNU Affero General Public License v3.0 (AGPL-3.0)**，
  详见 [LICENSE-AGPL-3.0](LICENSE-AGPL-3.0)。

完整说明见 [LICENSE](LICENSE)。
