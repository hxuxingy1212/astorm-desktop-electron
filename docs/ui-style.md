# UI 风格规范（渲染层唯一视觉权威）

> **给 agent 的读取约定**：任何渲染层 UI 改动（新增页面/组件、调样式、动主题变量）**开工前先读本文**，
> 按第 1–4 章的令牌与组件规范落地；第 5 章给出每个现有组件/页面对应的章节。
> 新增颜色、阴影、圆角、字号**一律走 token**，禁止在组件里引入新的硬编码值。
> **改对话正文/Markdown 相关的一律先读第 4 章**（它单独成章）。
>
> **状态标记**（第 3 章每节标题右侧）：
> - 🟢 **有参考图锚定** —— 值来自参考实现截图，可以直接照做。
> - 🟡 **部分参考** —— 参考图只覆盖了部分形态，缺失部分按同章推导规则做。
> - 🟠 **推导待确认** —— 参考实现里没有对应物，本文按既有规则推导出一套；落地时按此执行，
>   但**不要**当它是最终答案。第 6 章列了还缺什么参考。
> - 🔒 **形态冻结** —— 结构以仓库现有实现为准（不重画），只做令牌对齐（颜色/字体/字号/圆角/间距）。
>
> 参考基准：Codex macOS 客户端（ChatGPT 桌面端同源外观）：深色截图 5 张 + 浅色截图 4 张，
> 取值来源见附录 A。

---

## 0. 设计语言

**一句话**：单一强调色 + macOS 原生材质。层次由**面（surface）**表达而不是描边与投影；控件是填充/描边二选一的实心胶囊；
正文区去容器化，靠留白分区。**深色与浅色用两种不同的"面"语言**（见规则 2）。

### 7 条硬规则

1. **一个强调色**。强调色（`--accent-text`）只允许出现在 5 类元素上：主按钮、结构性选中环、
   开关选中态、结构性选项选中、输入聚焦光环。链接 / 文件引用文字改用 `--link-text`（浅色同强调色、
   深色取柔和浅蓝，见语义色表），不算强调色填充。其他地方一律中性灰。语义色只用于**表达状态**，不当装饰色。
   > 例外：**发送键 / 停止键是反色单色**，不是强调色（见 3.2）。
2. **面分两套语言，按模式切换**（这是本文最重要的一条，容易做错）：
   - **深色**：面 = **亮填充**（白 5% / 9% / 15%），**无描边**。
   - **浅色**：面 = **白底 + 1px 浅描边**（`rgba(0,0,0,.08)` 量级），**不是灰填充**。
   两套语言由同一组令牌 `--surface-bg` / `--surface-hover` / `--surface-border` 承载，组件只写令牌、不写模式分支。
   **但只适用于"抬升面"**（可交互容器）；代码块这类"凹陷面"浅色下也是填充，见第 2 章 L1a/L1b。
   `--fill-1/2/3` 继续承担 **hover / 按压 / 选中** 的阶梯（它们由 `--text-primary` 混合而来，两个模式都自动成立）。
3. **控件按用途分两种外形**：**输入/选择类**（输入框、文本域、下拉、色值 chip）= 面 + 描边；
   **动作类**（按钮）= 纯填充、无描边。同一个界面里两者并存是对的，不要统一成一种。
4. **浮层才用材质卡**。菜单、气泡卡、弹窗、命令面板、HUD 提示统一为「0.5px 发丝边 + 毛玻璃 `blur(20px) saturate(180%)`
   + 圆角 14px + 两级软投影」；浅色下同样成立（浅色是白底 96% + 描边 + 投影，毛玻璃只是点缀）。
5. **用户消息有容器，助手消息没有**。用户气泡右对齐、有底色；助手正文左对齐、通栏、直接落在画布上。
6. **双模式同步交付**。任何新增样式必须在浅色与深色下都成立，且**不要**用 `[data-theme='dark']` 分支去补
   ——优先用 `--surface-*` / `--fill-*` / `color-mix(var(--text-primary))` 这类自动跟随的写法。
7. **动效短促克制**。120–220ms、`cubic-bezier(0.32, 0.72, 0, 1)`，只做透明度与 ≤2% 缩放；必须处理
   `prefers-reduced-motion` 与 `prefers-reduced-transparency`（仓内已有先例，见 `styles/global.css:435,492`）。

---

## 1. 令牌层

自定义属性的**定义位置**（顺序即加载顺序，后定义覆盖前者）：

| 文件 | 职责 |
|---|---|
| `styles/themes/base.css` | 浅色令牌全表（`:root`） |
| `styles/themes/color-scheme.css` | 深色令牌（`[data-theme='dark']`）与 Arco 文字色 |
| `theme/tokens.css` | 主题色预设（blue/green/purple/orange/cyan，`body[data-theme-preset=...]`） |
| `styles/global.css` | 布局尺寸令牌（`--chat-column-max` 等）与 Arco 全局压平 |

**消费顺序**：组件里优先用项目令牌（`--surface-*`、`--fill-*`、`--text-*`…）→ 其次用 Arco 文字色
（`--color-text-1`…，主要在 teleport 到 body 的 Arco 弹层里用）→ 最后才考虑 Arco 原始类覆盖。
**永远不要**在组件 scoped 样式里写新色值。

### 1.1 字体

统一由 `--font-sans` / `--font-mono` 提供，定义在 `styles/themes/base.css` 的 `:root`（明暗通用），
并由 `styles/global.css` 的 `body` 落到全局（应用自身不再依赖 Arco 内置栈；那条栈里的 `Inter` **未打包**）。

```css
--font-sans:
  system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC',
  'Hiragino Sans GB', 'Microsoft YaHei', sans-serif;
--font-mono:
  'SF Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, 'Cascadia Code', monospace;
```

- **直接使用操作系统默认 UI 字体**（`system-ui`）：macOS → SF Pro，Windows → Segoe UI；中文由系统字体回落（PingFang SC / 微软雅黑）。**不换字体**，也不引入 Web 字体。
- 等宽五套并存（Cascadia / SF Mono / ui-monospace / Menlo）→ 收敛到 `--font-mono` 一条。
- 参考实现里等宽字体用在：正文行内代码、代码块、**工具执行行**（`Ran …`）、命令块。
- 不引入 Web 字体，不做 `@font-face`（离线可用 + 启动无 FOIT）。

**字号刻度**（令牌见 `styles/themes/base.css`，**只允许用这几档**）：

| 令牌 | px | 用途 |
|---|---|---|
| `--font-size-2xs` | 11 | 极小的角标/计数（少量，谨慎用） |
| `--font-size-xs` | 12 | 分组标签、快捷键提示、时间戳、分区小标题 |
| `--font-size-sm` | 13 | 次级说明、菜单项、chip、状态行、工具执行行 |
| `--font-size-md` | **14** | UI 默认：导航项、列表项、标签页、输入框，**以及会话正文** |
| `--font-size-title` | 15 | 设置行标题、卡片标题（h4 同档） |
| `--font-size-lg` | 17 | 卡片/区块标题（h2 同档） |
| `--font-size-xl` | 20 | 区块大标题（h1 同档，页面内少用） |
| `--font-size-2xl` | 28 | 页面级大标题（设置页标题、视图页标题） |
| `--font-size-code` | 12.5 | 代码 / 命令 / 路径（配 `--font-mono`；文件预览同档） |

> **令牌化已铺开**：全仓 600+ 处 `font-size` 已收敛到上表（`--font-size-*`），
> 硬编码 px 只剩两处例外：引导页标题（`onboarding/` 有独立视觉约定）与一个用 font-size 当图标尺寸的
> chevron（`ToolsSettings.vue:440`，属图标用法，应换 SVG，见 1.6）。

> 参考实现（Codex）的刻度：其设计令牌是 `text-xs/sm/md/lg` = 12/14/16/18，**渲染实测**（同机 DPR 2、
> 窗口 1090×760 截图）——侧栏项与会话正文的汉字中心距 27.8/28.0 物理 px（÷2 = **14.0 CSS px**），
> 代码行字符步距 7.51 CSS px（÷0.6em = **12.5px**）、代码行距 20 CSS px（= **1.6 倍**）。
> 它比常见 macOS 界面整体大一档，**这也是它显得"从容"的原因**——不要按 12/13px 做设置页。

**行高**（令牌 `--line-height-*`）：UI 控件 1.2（`--line-height-ui`，菜单文字采用紧凑行高）；正文 **1.5**（`--line-height-body`，按本地 Codex 视觉校准）；代码 **1.6**（`--line-height-code`，实测 20px/12.5px）。
**字重**：只用 `--font-weight-regular/medium/semibold`（400/500/600）。不用 700（中文粗体在 macOS 上会糊）。
**字距**：28px 标题 `-0.01em`，其余 0。

### 1.2 色板

#### 画布

| 令牌 | 浅色 | 深色 | 用途 |
|---|---|---|---|
| `--bg-base` | `#ffffff` | `#181818` | 画布：页面底、正文区、面板底、弹窗实底 |
| `--panel-bg` | `#ffffff` | `#181818` | 右侧面板/对话区底（**与画布同底**，不用纯黑） |

参考实现**自报**的两组值（外观设置页可见，可信度最高）：

| 模式 | Background | Foreground |
|---|---|---|
| 浅色 | `#FFFFFF` | `#1A1C1F` |
| 深色 | `#181818` | `#FFFFFF` |

本项目取值：浅色 `--bg-base: #ffffff`（一致）、`--text-primary: #1a1c1f`（对齐）；
深色 `--bg-base: #181818`（**现状是 `#141414`，要改**）、`--panel-bg: #181818`（**现状纯黑 `#000000`，要改**）、
`--text-primary: #dedfe2`（2026-10-19 降低字体亮度，近黑底上采用柔和浅灰）。

#### 面（surface）与交互填充

```css
/* 静态面：卡片、输入框、下拉、chip、菜单、composer —— 组件只写这三个 */
--surface-bg:     浅 #ffffff          深 var(--fill-1)
--surface-hover:  浅 var(--fill-1)    深 var(--fill-2)      /* 悬浮加深（浅色下白→浅灰，深色下亮一档） */
--surface-border: 浅 1px solid #e5e6eb  深 1px solid transparent
--fill-1/2/3:     由 --text-primary 混合 5% / 9% / 15%（两模式自动翻转），用于 hover / 按压 / 选中 */
```

- `--surface-border` 是**完整 border 值**（含 `1px solid`），直接 `border: var(--surface-border)`。
  深色下它是 transparent 而不是 none —— **保留 1px 占位，避免模式切换时布局跳动**。
- `--fill-*` 保持现状不变：它已经是"跟随主题的透明度阶梯"，且在深色下正好就是面的填充。
- 深色下 `--surface-bg` 直接用 `--fill-1`，于是深色完全延续现有观感，改动只落在浅色。

#### 文字

| 令牌 | 浅色 | 深色 | 用途 |
|---|---|---|---|
| `--text-primary` | `#262626` | `#cdcdcd` | 标题、正文 |
| `--text-title` | `#181818` | `#ededed` | 顶栏会话标题（比正文更深/更亮一档） |
| `--text-body` | `#262626` | `#cdcdcd` | 会话正文（深色与主文字同档） |
| `--text-secondary` | `#5d5d5d` | `#b4b4b4` | 说明文字、次级值、菜单项 |
| `--text-tertiary` | `#8a8a8a` | `#8a8a8a` | 占位符、时间戳、行号、分隔标签、工具执行行 |
| `--text-disabled` | `#c9cdd4` | `#4e5969` | 禁用 |

#### 语义色

| 令牌 | 浅色 | 深色 | 用途 |
|---|---|---|---|
| `--accent-text` | `#165dff` | `#306fff` | 强调色（见规则 1） |
| `--link-text` | `#165dff` | `#a0c7f4` | 链接与文件引用文字（深色用柔和浅蓝） |
| `--success` | `#00b42a` | `#34c759` | 成功、新增行、通过 |
| `--warning` | `#ff7d00` | `#ff9f0a` | 警告、放开权限 |
| `--danger` | `#f53f3f` | `#ff453a` | 危险、删除行、拒绝、错误 |
| `--info` | `var(--accent-text)` | 同 | 中性提示 |

**裁决：语义色不再区分"macOS 系 / Arco 系"。** 现状两套色值在仓里混用（macOS `#34c759` 硬编码 16 处、
`#ff3b30` 5 处；Arco `--success`/`--danger` 走 65+ 处），必须收敛到上表：浅色沿用既有 Arco 值
（已按 Arco 调校、对比度合格、零回归），深色补齐 macOS 暗色系值（**现状深色没有重定义语义色，
在 `#181818` 上偏暗，这是真实缺陷**）。**禁止**再写 `#34c759` / `#ff3b30` / `#00b42a` / `#1677ff` 等裸色值。

#### 图标与文件图标 → **见 1.6**

图标是**受控目录**（五个族、十个分类、常见选错对照、新增流程都在 1.6）：**只能从目录里挑，不许随手选**。
一句话版本：界面控件一律用手写 **UI 图标**（`assets/ui-icons/`，`currentColor` 单色，随 hover/选中态变色）；
`i-lucide-*` 与 lucide/vscode-icons 是**遗留/已停用**，不要新增；品牌图标只用于邮件发件人识别。

### 1.3 形状

```css
--radius-xs: 4px;    /* 复选框、色块、极小小件 */
--radius-sm: 6px;    /* chip、图标钮、菜单项、行内代码 */
--radius-md: 8px;    /* 标准控件：按钮、输入、下拉、页签、缩略图 */
--radius-lg: 12px;   /* 卡片、下拉菜单、气泡卡 */
--radius-xl: 14px;   /* 材质卡：弹窗、HUD、抽屉、命令面板、设置卡片 */
--radius-2xl: 18px;  /* 大容器：composer 输入卡 */
--radius-full: 999px;/* 胶囊：搜索框、chip、开关、徽标 */
```

> 现状只有 4/8/12 三个 + 散落的 7/9/10/14px 硬编码（`mac-modal.css:89`、`settings.css:27`、`inputs.css:25`…）。
> 新代码用上表，触及旧处时顺手收敛。

```css
--hairline: 0.5px solid color-mix(in srgb, var(--border-base) 70%, transparent);
--material-blur: blur(20px) saturate(180%);

--shadow-sm:      0 1px 2px rgba(0, 0, 0, 0.08);
--shadow-md:      0 4px 16px rgba(0, 0, 0, 0.10), 0 1px 2px rgba(0, 0, 0, 0.06);
--shadow-lg:      0 14px 40px rgba(0, 0, 0, 0.20), 0 1px 2px rgba(0, 0, 0, 0.08);
--shadow-material: var(--hairline), var(--shadow-lg);  /* 材质卡 = 发丝边 + 大投影 */
```

**阴影只给三种东西**：`--shadow-sm` 贴附元素（发送键、开关手柄）、`--shadow-md` 悬浮卡/菜单、
`--shadow-lg` 模态浮层。平面卡片**零投影**（浅色下它靠描边、深色下靠亮填充）。

### 1.4 间距与尺寸

- 基准 4px 网格，可用步长：`4 / 6 / 8 / 12 / 16 / 20 / 24 / 32`。
- 常用高度：菜单项 28（`--menu-item-height`）、侧栏/设置导航 32（`--nav-item-height`）、图标钮 28、控件/行 32、设置行 56（两行文本 68）、顶栏 44（全屏 52）、分区分隔 24。
- **默认窗口尺寸 1090×760**（最小 780×600，`main/index.ts` 的 `createWindow`）：按参考实现对齐 ——
  在 16″ 屏（2x）上截参考窗口，量得窗口 2180×1520 物理 px = **1090×760 CSS px**。
  原先的 1280×800 明显偏大：宽窗口会让设置页内容贴左、聊天列两侧留白过多。
  > 量法（可复现）：截图为「窗口 + 阴影」四周留黑，取亮度 > 阈值像素的外接框即窗口矩形；
  > 同一批截图里 macOS 红黄绿三个按钮都是 28 物理 px（= 14pt@2x），可用它核对截图倍率。
- **滚动条：4px 细条 + 空闲不显示 + 滚动时才出现 + 始终占位**。全应用**唯一一处**定义在
  `styles/global.css`，行为在 `services/scrollbars.ts`（挂载后装一次），组件里**不要**再写
  `scrollbar-width` / `scrollbar-color` / `::-webkit-scrollbar` —— 那会切到 Chromium 原生条
  （macOS 是不占位的叠加条、Windows 是 11px 粗条），站内立刻出现第二套粗细与显示行为。
  > - 显示时机：滚轮 / 触控 / 滚动键 / 拖 thumb 才算「用户滚动」（`.is-scrolling`，滚完约 1.2s
  >   淡出）；程序化滚动（流式回答自动到底、`scrollIntoView`、开面板恢复位置）不显示，否则
  >   回答期间滚动条长亮。Arco 自绘的那套（下拉/选择器弹层，默认 15px 常亮）也在 global.css
  >   里并到了同一形态与时机。
  > - 占位：滚动容器由 JS 打 `data-scroll-gutter`（= `scrollbar-gutter: stable`），内容不因
  >   滚动条出现/消失左右移位。**不能改写成全局 `*` 规则**：实测 Chromium 对 `overflow: hidden`
  >   的裁剪容器也生效，全局写会把所有圆角/截断容器的内容缩掉 4px。
  > - 两个反向特例：页签条 `.tabbar-list` 与看板 `.board-columns` 的**横向**滚动条整体隐藏 ——
  >   横向槽占的是高度，`scrollbar-gutter` 只管纵向槽，一显示必然压掉内容高度，与「必须占位」冲突。
- **原生外观跟随应用主题**：渲染层 `applyTheme()` 每次应用主题都把模式推给主进程
  （`appearance:set-theme-mode` → `nativeTheme.themeSource`）。macOS 的红黄绿**失焦色**、
  原生菜单与原生弹窗外观都由窗口外观决定 —— 不同步时「系统浅色 + 应用深色主题」会把失焦的
  三个点画成近黑（在深色标题栏上等于消失）；同步后深色主题下是浅灰（实测 ≈ rgb(73,73,73)，与参考实现一致）。
- 内边距：卡片/弹窗 16、页面 24（窄窗 16）、列表行水平 8–12。
- 对话内容列：`--chat-column-max: 960px`，宽窗口下居中并留白（`styles/global.css:59-84`）。
  > 刻意与参考实现偏离：参考的正文无宽度上限（长行阅读吃力）。**保留 960px 上限**。
- 拖拽区：容器级 `app-region-drag`，任何浮层/按钮必须显式 `app-region-no-drag`
  （`styles/global.css:29-49` 有长注释，别删）。

### 1.5 动效

```css
--transition-fast:   0.15s ease;
--transition-normal: 0.3s ease;
--transition-panel:  0.1s ease;   /* 面板拖拽/伸缩，跟手 */
--ease-apple:        cubic-bezier(0.32, 0.72, 0, 1);
```

| 场景 | 时长 / 曲线 | 表现 |
|---|---|---|
| hover / 底色变化 | 150ms ease | 仅颜色 |
| 按压 | 100ms | `scale(0.97)`（按钮类） |
| 浮层入场 | 220ms ease-apple | `opacity 0→1` + `translateY(-6px)→0` + `scale(0.98)→1` |
| 折叠/展开 | 200ms ease | 高度或透明度，不做弹性回弹 |
| 面板伸缩 | 100ms | 无过渡动画，直接跟手 |

必须同时提供 `prefers-reduced-motion: reduce`（关缩放与位移动画）与 `prefers-reduced-transparency: reduce`（关毛玻璃）降级。

### 1.6 图标（受控目录）

**硬规则：图标只能从目录里挑，不许按形状联想随手选。**

图标是"廉价但显眼"的元素——同一语义用了两个图标，界面立刻显得不专业。为此本项目建了**受控目录**：
每个字形都有名字、有分类、有一句"什么时候用"。挑图标前先看目录。

**三个入口（内容同源，改图标先改单一来源）**：

| 入口 | 用途 |
|---|---|
| **`#/icons` 图标总览页** | **挑图标就看这里**：按分类陈列（亮/暗并排）、带"什么时候用"与「常见选错」对照 |
| `utils/uiIconCatalog.ts` | 分类 / 提示 / 易错对照的**单一来源**；漏归类会在模块加载期抛错 |
| 本节 | 速查表与规则；新增或替换图标后同步本节 |

**五个图标族**（先选族，再选字形）：

| 族 | 位置 / 用法 | 数量 | 什么时候用 |
|---|---|---|---|
| **UI 图标**（手写单色） | `assets/ui-icons/*.svg`；模板 `<icon-名称/>`、JS `appIcon('icon-名称')`、底层 `<AppIcon glyph="名称" />` | 108 字形 / 121 注册名 | 所有界面控件、按钮、状态 —— **默认用这一族** |
| **文件类型图标** | `assets/file-icons/*.svg` → `fileGlyphUrl()`；知识库用 `MacFileIcon` | 64 | 只在"表示这是什么文件"时用（文件树 / 页签 / 提及 / 预览头部） |
| **品牌图标** | `components/mail/mailBrandIcons.ts`（脚本生成，勿手改） | 84 | 只在邮件发件人识别时用；**别处当 UI 图标用会像广告位** |
| UnoCSS `i-lucide-*` 类 | 直接写类名 | 10（**遗留**） | **禁止新增**；改到哪个组件就顺手换成 UI 图标 |
| lucide / vscode-icons（历史对照） | 仅 `#/icons` demo 页 | — | 已停用；业务代码里出现即视为 bug |

**字形约定**：48 网格 / 源文件 `stroke-width: 4` / `currentColor`（随 hover 与选中态自动变色，**不要写死颜色**）/ 圆角端点。
运行时线宽以 `UI_ICON_STROKE_WIDTH = 1.8`（lucide 24 网格语义，`<AppIcon>` 内部 ×2 换算）为**全局唯一标准**；
尺寸只用 14 / 16 / 18 / 20，**不要逐个改线宽**（要改就改这个常量；字形文件保持 48 网格 `stroke-width="4"`）。

**多色的三处例外**（属"内容色"，不算 UI 装饰色、也不受规则 1"一个强调色"约束）：
文件类型图标（`--ficon-*`）、品牌图标、变更统计（`+98 -36`）这类按数据着色的元素。

**十个分类**（挑图标：先定位分类，再看"什么时候用"；`code` 里是字形名）：

**导航与方向 `nav`** —— 表达"往哪去 / 展开收起"
`chevron-down`(下拉、可展开项) · `chevron-up`(收起、折叠) · `chevron-left`(返回/上一页) ·
`chevron-right`(进入下一级、树节点折叠态) · `arrow-up`(上传、置顶、回到顶部) ·
`arrow-down`(向下方向，备用字形) · `external-link`(在外部/浏览器打开) · `compass`(探索、发现)

**窗口与布局 `chrome`** —— 面板与视图形态、窗口控制、拖拽把手、指针
`panel-left`(侧栏展开/收起) · `panel-right`(面板/页签区伸缩) · `expand`(放大/最大化/全屏) ·
`shrink`(缩小/还原) · `more-horizontal`(更多操作、行内菜单) · `grip-vertical`(拖拽排序把手) ·
`square`(停止、占位方块) · `monitor-smartphone`(响应式预览/自由尺寸) · `kanban`(看板视图) ·
`layers`(层级/堆叠/分组) · `mouse-pointer`(悬停/指针态) · `mouse-pointer-click`(拾取元素)

**动作与操作 `action`** —— 用户可点的动词
`plus`(新建/添加) · `minus`(减少/移除/收起) · `close`(关闭/取消) · `copy`(复制) · `save`(保存) ·
`trash`(删除，不可逆) · `archive`(归档) · `download`(下载/导出) · `send`(发送) · `pencil`(编辑/重命名) ·
`brush`(样式调整/绘制) · `eraser`(清除内容) · `wrench`(工具/修复) · `hammer`(构建/生成) · `search`(搜索) ·
`filter`(筛选) · `refresh`(刷新) · `refresh-ccw`(同步/循环) · `rotate`(重试/旋转/回复) · `play`(运行/开始) ·
`pin`(置顶) · `pin-off`(取消置顶) · `redo`(重做，与 undo 成对) · `wrap-text`(自动换行) · `eye`(预览/显示隐藏) · `link`(绑定/关联) ·
`unlink`(解绑/断开) · `star`(收藏/星标)

**状态与反馈 `state`** —— 表达"结果 / 属性"，不是动作
`check`(勾选/完成) · `check-circle`(成功状态) · `alert-circle`(警告/错误) · `info`(提示/说明) ·
`loading`(进行中，可加 `.spin`) · `bell`(通知/提醒) · `shield-check`(安全/已防护) · `lock`(锁定/受限) ·
`zap`(快速/即时) · `gauge`(用量/配额/仪表)

**文件与目录 `file`**
`file`(未知类型兜底) · `file-text`(文本文档) · `file-spreadsheet`(表格文件) · `folder`(文件夹) ·
`folder-plus`(新建文件夹) · `folder-tree`(目录树/心智图) · `clipboard`(剪贴板/清单) · `box`(打包产物)

**内容与媒体 `content`**
`book`(文档/手册/知识) · `quote`(引用原文) · `library`(资料库/素材库) · `tags`(标签/分类) · `image`(图片/媒体) ·
`video`(视频) · `camera`(截图/拍照) · `chart-bar`(统计/图表/用量) · `table`(表格视图/演示稿)

**会话与时间 `session`**
`message`(对话/消息) · `mail`(邮件) · `inbox`(收件箱/空态) · `clock`(时刻) · `history`(历史/会话轨迹) ·
`calendar`(日期/日程) · `calendar-clock`(定时任务) · `summary`(摘要) · `think`(思考链)

**智能体与身份 `agent`**
`bot`(智能体/子会话) · `users`(成员/协作) · `id-card`(身份/账号/凭证) · `brain`(长期记忆) ·
`puzzle`(插件) · `sparkles`(技能/AI 生成)

**系统与环境 `system`**
`settings`(设置页) · `settings-2`(配置项/参数) · `code`(代码/源码) · `code-square`(代码块/文件预览) ·
`terminal`(终端/命令行) · `database`(数据/存储) · `cpu`(模型/算力/推理) · `globe`(网页/站点/网络) ·
`help`(帮助/字段说明) · `git-branch`(分支) · `git-compare`(变更对比) · `github`(GitHub)

**斜杠命令专用 `command`** —— 只给提及编辑器的 `/` 命令用，别拿去当普通图标
`undo`(/undo) · `goal`(/goal) · `init`(/init) · `plan`(/plan) · `compact`(/compact) · `fork`(/fork)

**常见选错对照**（挑之前扫一眼；完整版在 `#/icons`）：

| 该用 | 别用 | 为什么 |
|---|---|---|
| `chevron-down` | `arrow-down` | 展开/收起一律 chevron；arrow 只表示方向本身 |
| `settings` | `settings-2` | 设置页入口 vs 单个配置项 |
| `file` | `file-text` | 未知类型兜底 vs 已知是文档 |
| `refresh` | `refresh-ccw` | 可点的"刷新" vs "同步/循环" |
| `loading` | `refresh` | 进行中（不可点）vs 可点动的刷新 |
| `clock` | `history` | 具体时刻 vs 历史列表/轨迹 |
| `alert-circle` | `info` | 警告错误 vs 中性说明 |
| `check` | `check-circle` | 勾选态 vs 整块操作的成功结果 |
| `trash` | `close` | 删除 vs 关闭 —— 误用会让人不敢点 |
| `square` | `play` | 停止 vs 运行 |
| `bot` | `users` | 智能体 vs 人与成员协作 |
| `puzzle` | `sparkles` | 插件 vs 技能/AI 生成 |
| `folder` | `folder-tree` | 普通目录 vs 目录树/心智图 |
| `terminal` | `code` | 终端 vs 代码；代码块与预览用 `code-square` |
| `globe` | `external-link` | 网页/站点本身 vs "在外部打开"这个动作 |

**新增 / 替换图标**（四步，第 2 步漏了会在加载期抛错）：

1. **字形**：把 SVG 放进 `assets/ui-icons/`（48 网格、`stroke-width="4"`、`currentColor`、圆角端点；运行时线宽由 `UI_ICON_STROKE_WIDTH` 统一覆盖）。
2. **登记分类**：`utils/uiIconCatalog.ts` 的 `GLYPH_CATEGORY` 与 `GLYPH_HINT` 各补一行。
3. **注册名**：`utils/uiIconGallery.ts` 的 `NAME_TO_GLYPH` 加 `icon-xxx` → 字形（模板与 JS 侧可用的别名）。
4. **同步**：`#/icons` 自动生效；本节表格补上该字形。

**替换既有图标**只需改 `NAME_TO_GLYPH` 的一行（调用方零改动）——**但不要用"替换"来引入风格不一致的字形**，
那是把自己变成第二个图标来源。

**待收敛**：生产代码里只剩 **1 处** `i-lucide-*`（`layouts/AppSidebar.vue:795` 的项目文件夹图标，用 `i-lucide-folder` /
`i-lucide-folder-open`），它的线宽与端点跟手写字形不一致，改到时换掉（"打开态"目前没有对应字形，可补一个或改用展开旋转）。
`#/icons` 里另外登记的 10 个类名只是 demo 候选；`uno.config.ts` 的 safelist 指向 `utils/legacyVscodeFileIcons.ts`
（86 行历史对照表，仅供总览页的「替换前」演示）——**这两处都不是生产用法，不要照抄**。

### 1.7 共享组件与复用规则

> **为什么放在视觉规范里**：这个项目的历史债务大多是"同一个东西被写了三遍"——同一段设置行 CSS 在 5 个页面字节相同、
> 同一段悬浮卡定位代码在 2 个组件里逐行同构、12 种等宽字体写法。视觉不一致只是症状，**根源是复用没有被制度化**。
> 所以"该用什么"和"颜色该用什么"同样是规范的一部分。

**五条硬规则**：

1. **先查、再抽、后写**：新写一个 UI 小件前，先在本节找一遍。
2. **共享层没有的，先加共享件、再在页面里用**——不要"先在页面里写，等第二个页面再抽"。
3. **同一个视觉形态在第二个页面出现，就是抽出来的信号**。
4. 共享件不够用时**扩展它**（加修饰类 / 加 prop / 加 slot），不要在页面里复制一份改。
5. 改完自查有没有"本该复用"的东西又被写了一遍。（原 `check:reuse` 棘轮脚本随 2026-10-15 回滚移除，规则仍在本节。）

**索引表**（"我要做 X" → 用什么。★ = 规范已定、**尚未落地**，落地计划见 `docs/ui-dedup-plan.md`）：

| 你要做的事 | 用什么 | 在哪 |
|---|---|---|
| 放一个图标 | `<AppIcon glyph="…">` / `<icon-注册名/>` | §1.6 受控目录 |
| 表示"这是什么文件" | `fileGlyphUrl(name)` / `<MacFileIcon>` | §1.6 |
| 输入框 / 文本域 / 下拉 / 数字框 | **不用做** —— `styles/inputs.css` 已全局压平 Arco | `styles/inputs.css` |
| 大段文档输入（约定 / 智能体指引 / 技能指令 / 定时任务指令） | `<MultilineTextField>`（描边 + 原生纵向缩放柄） | `components/common/MultilineTextField.vue` + `styles/inputs.css` 第 5 节 |
| 卡片 / 列表项 / 工具项 | **不用做** —— 把类名加进 `cards.css` 的名单即可 | `styles/cards.css` |
| 工具卡片（名称 / 描述 / 参数数 / 来源，复制·详情） | `<ToolCard>` | `components/tools/ToolCard.vue` |
| MCP 服务器卡片（连接 / 启停 / 工具数，点击下钻工具页） | `<McpServerCard>` + `<ToolCard selectable>` | `components/tools/McpServerCard.vue` |
| JSON 配置编辑（安全 / MCP，页内非弹窗） | `<JsonEditorPane>` | `components/common/JsonEditorPane.vue` |
| 本地配置文件入口（路径 + 打开按钮） | `<SettingsConfigFileRow>` | `components/settings/SettingsConfigFileRow.vue` |
| 邮箱 / 模型的列表与详情编辑器 | `settings-editor-*` 共享配方 | `styles/settings.css`（结构、列表、表单）、`styles/cards.css`（卡片面） |
| 设置页分组（标题在框外左上、内容常展开） | `<SettingsSection>` | `components/settings/SettingsSection.vue` + `styles/settings.css` |
| 设置页的一行（标题 + 说明 + 右侧控件） | `styles/settings.css` 的行配方 · ★ `<SettingRow>` | `styles/settings.css` |
| 弹窗 | `<a-modal modal-class="mac-modal">` | §3.3 |
| 任务型对话框（导入 / 清理 / 批量确认） | 大标题对话框变体 | §3.3 |
| 确认 / 警告（命令式） | `useMacConfirm()` | `composables/useMacConfirm.ts` |
| 悬浮卡 / 气泡卡 | `<HoverCard>` | §3.3 |
| 对话框工具行图标按钮（计划 / 目标 / 子会话，图标 + 计数） | `<FooterIconButton>` | `components/chat/FooterIconButton.vue` |
| ★ 任意浮层的"点外关闭" | ★ `useClickOutside()` | 方案 P1 |
| ★ 定位到某个锚点的浮层 | ★ `useAnchoredPanel()` | 方案 P1 |
| 空态 | ★ `<EmptyState>` | 方案 P2 |
| 加载中的旋转 | ★ `.spin`（全局工具类） | 方案 P0 |
| Markdown 正文 / 节点渲染器 | `<MarkdownBody>` + `components/markdown/renderers.ts` | 第 4 章 |
| 代码块（只读） | `<CodeBlock>` | §3.5 |
| 代码编辑器 | `<CodeEditor>` | §3.5 |
| 文件预览弹窗 | `<FilePreviewModal>` | §3.5 |
| 选目录 | `useDirectoryPicker()` | `composables/` |
| 拖拽改宽 | `useResizeDrag()` | `composables/` |
| ★ 列表拖拽排序 | ★ `useListDragSort()` | 方案 P1 |
| ★ 文件树（展开 / 过滤 / 选中） | ★ `useFileTree(loader)` + `utils/fileTree.ts` | 方案 P2 |
| ★ 写剪贴板 | ★ `copyText()` | 方案 P0 |
| 时间 / 时长格式化 | `utils/time.ts`（`relativeTime` / `formatDateTime` / `formatDuration`） | `utils/time.ts` |
| ★ 文件体积格式化 | ★ `formatSize()` | 方案 P0 |
| ★ 取错误信息 / 提示用户 | ★ `errorMessage()` + `notify.error()` | 方案 P0 |
| ★ 判断当前是否深色 | ★ `useIsDark()` | 方案 P0 |
| 开页签 / 开文件页签 | `tabs.openTab()` / `tabs.openFileTab()` | `stores/tabs.ts` |
| ★ 跳转到一个会话 | ★ `sessions.openSession(id)` | 方案 P1 |
| 窗口断点 / 面板列宽 | `useBreakpoint()` / `usePanelLayout()` | `composables/` |
| 分段控件 / 选项组（多选列表） | `components/common/SegmentedControl.vue`（`size="md"` 页面级 / `size="sm"` 菜单内）；选项组见 §3.4 规格 | §3.4 |
| 深色适配的一层浮层样式 | 材质卡配方（`--hairline` + 毛玻璃 + 两级软投影） | §2 L3 |

**明确不要抽的**（审计已确认，避免过度抽象）：数据可视化与专属图形（提交图谱 / 甜甜圈 / 文件夹造型 / 交通灯）、
富文本编辑器的对齐与列表图标（无对应字形）、composer 的实心发送箭头（与 `send.svg` 造型语言不同）、
`MailCompose` 的大标题骨架（有意的体量差异）、三个走 Arco `a-popover` 的胶囊卡（已一致）、
`ChatPanel` 的错误浮条（全应用唯一一处）。完整理由见 `docs/ui-dedup-plan.md` §4。

**当前重复量**（2026-10-15 审计）：259 处可机械识别的重复，
其中等宽字体字面量 90、内联错误取值 62、内联 `<svg>` 32。收整计划与分批方式见 `docs/ui-dedup-plan.md`。

---

## 2. 层级与材质模型

从底到顶只有 5 档，**不允许发明第 6 档**：

| 层 | 浅色 | 深色 | 典型 |
|---|---|---|---|
| L0 画布 | `--bg-base` 白 | `#181818` | 对话区、面板、设置页内容区 |
| L1a 抬升面 | **白底 + `--surface-border` 描边** | **`--fill-1` 亮填充、无描边** | 可交互容器：卡片、输入框、下拉、chip、次级按钮、composer |
| L1b 凹陷面 | **`--fill-1` 填充、无描边** | `--fill-1` 填充、无描边 | 内容井：代码块、命令块、日志/输出区、面包屑条、frontmatter |
| L2 面 | `--fill-1`（浅灰） | `--fill-2` | hover、用户气泡、选中行 |
| L3 浮层（材质卡） | 白 96% + 毛玻璃 + 描边 + `--shadow-md` + `--radius-lg/xl` | `--bg-1` 88% + 毛玻璃 + 发丝边 + 同投影 | 菜单、气泡卡、HUD、悬浮卡 |
| L4 模态 | 遮罩 `rgba(0,0,0,.28)` + 材质卡 + `--shadow-lg` + `--radius-xl` | 遮罩 `.45` + 同上 | 弹窗、确认卡、命令面板 |

- **抬升面 vs 凹陷面**（容易判错，务必分清）：**可交互的容器**用抬升面（浅色下白底 + 描边），
  **只放内容、不直接交互的"井"**用凹陷面（浅色下也是灰填充、无描边）。参考实现里代码块就是凹陷面
  ——所以它在浅色下**不是**白底 + 描边，别照搬规则 2 到代码块上。
- **材质的"底"用 `--bg-1`/`--bg-2` 半透明而不是 `--panel-bg`**：深色下 `--panel-bg` 若为纯黑，
  实心材质卡会发硬（`global.css:378` 注释有同样的结论）。
- 浮层里再套卡片时，内层卡片改用 `--fill-2`（避免同色糊在一起）。
- 遮罩只压暗、不做模糊。
- **侧栏与设置左栏是固定实色**（不在上述 5 档内）：为保证窗口/全屏/动画快照颜色一致，见 3.1。

---

## 3. 组件规范

### 3.1 应用外壳

#### 侧栏底色（固定实色；2026-10-20 修订原「毛玻璃」决策）

参考实现的侧栏/设置左栏是透出桌面的 macOS 毛玻璃。本项目曾开启原生 vibrancy，但
**macOS 全屏动画会先取窗口快照再切换 vibrancy 材质**，侧栏会把桌面/壁纸透出来；Electron
只暴露动画结束后的 `enter-full-screen`，无法在快照前切色，展开过程中必然看到颜色变化。
为保证窗口、全屏与动画三态颜色一致，侧栏改为**固定实色**：

1. 主进程仍保留 `vibrancy` / `backgroundMaterial`（窗口其他透明面与圆角可用），
   但侧栏不再依赖它：`.nav-sider.native-glass` 固定 `background-color: var(--sidebar-solid-bg)`。
2. `--sidebar-solid-bg` 是侧栏唯一底色令牌：深色 `#2e2e2e` / 浅色 `#f7f8fa`
   （取原窗口模式玻璃层的等效观感，见 `themes/base.css` / `themes/color-scheme.css`）。
   非原生平台（Linux / 浏览器）继续用 `--bg-1` 实底。
3. 侧栏与中心区无缝相接，不画边框、不留间隙；调整侧栏宽度使用交界处的透明拖拽热区（2026-10-19 确认）。
4. 侧栏展开/收起直接切换目标宽度，不对 width/min-width 做过渡，避免看板和邮箱在伸缩期间逐帧重排；分栏拖拽每帧合并一次更新（2026-10-20）。
5. 若未来恢复毛玻璃，必须先拿到原生 `willEnterFullScreen` 时机（Electron 未暴露）或接受全屏动画期间透色。

#### 顶栏 / 状态栏 `layouts/StatusBar.vue` 🟢

- 高 44px（全屏 52px），横向：macOS 红绿灯占位（`pl-[76px]`）→ 页签条 → 右侧工具图标区 → 窗口控制（非 mac）。
- 图标钮：28×28、`--radius-md`、`currentColor`（`--text-secondary`），hover `--fill-2` + `--text-primary`。
- 标题：14px / 500 / `--text-primary`；次级 meta chip 12px / `--text-tertiary`。
- 整条是窗口拖拽区，**条内每个可点元素都要 `app-region-no-drag`**。

#### 分裂按钮 🔒已拍板（要引入）🟢

参考实现右上角的 `Open ⌄` 是同色分裂按钮：主段是默认动作，右段固定宽度只放 chevron，点开是动作菜单。
**本项目确认引入**（2026-10-15 拍板），作为新的通用控件（如"在浏览器中打开 ⌄"、"提交 ⌄"这类多入口动作）。

- 外形：高 28/32px、`--radius-md`、两段共用同一底色；主段内边距 0 12px、**右段定宽 24px 且只放 12px chevron**。
- 两段之间一条 1px 分隔：强调色按钮用 `rgba(0,0,0,.18)`，中性按钮用 `--border-base`。
- 变体：
  - **主色**：底 `--accent-text`、白字白图标；hover 整段 `brightness(1.06)`；分隔线处 hover 只加深分隔段（`rgba(0,0,0,.10)`）。
  - **中性**：底 `--surface-bg` + `--surface-border`，hover 用 `--surface-hover`（浅色可作默认）。
- 交互：主段点击 = 默认动作；右段点击 = 菜单，菜单**左对齐右段**、距按钮底 4px；键盘 `Enter` 触发主段、`Alt+↓` 开菜单。
- 只在一屏有"一个主动作 + 若干次要入口"时用；两个并列动作请用两个普通按钮。

#### 页签条 `components/panels/PanelTabBar.vue` 🟢

- 浏览器页签统一使用手写 `globe` 图标，不展示站点 favicon；不提供页签历史下拉，也不记录最近打开/关闭列表（2026-10-19）。
- 页签：高 30px、`--radius-md`、内边距 8/10px、13px 文字。
- 未激活：透明底 + `--text-secondary`；hover：`--fill-1`。
- 激活：`--fill-2` + `--text-primary`；关闭 × 只在 hover/激活时浮现（12px，`--text-tertiary`）。
- 溢出：横向滚动条整体隐藏（特例，见 §1.4）；页签右侧 + 新建。
- 标题溢出：悬浮时用 `scrollLeft` 跑马灯（≤8px 不滚、约 30px/s、左侧 12px 渐隐），
  与侧栏任务行共用 `useHoverMarquee`。

#### 左栏 `layouts/AppSidebar.vue` 🟢

- 宽 240–260px；底为固定实色（见上）；右分隔 `1px var(--border-light)`。
- 品牌行（高 44/52px）：logo 34px + 标题 17px/600；mac 下左内边距 76px 让开红绿灯。
- 导航项：高 32px、`--radius-md`、图标 18px（`--text-tertiary`）+ 文字 14px；
  激活/hover = `--fill-2`（实底层上的填充要比底色深一档才看得出来），**不加左侧色条、文字不染强调色**。
- 分组标签（会话/任务/最近）：13px / `--text-tertiary`，上间距 16px。
- 树形行（项目/会话）：缩进 24px、行高 30px、选中 `--fill-2`；不画连接引导线。
- **会话行悬浮**：800ms 后在行右侧弹出 `HoverCard`（完整标题 / 最近活动 / 状态 / 工作区路径），
  无箭头；不显示行内悬浮操作图标（2026-10-20 产品决定）。标题溢出 ≤8px 不滚，超出按约 30px/s
  跑马灯，日期列保持不动。
- **运行中的会话行**：行尾放一个 12px 环形 spinner（`--text-tertiary`，1.5px 描边，1s 线性旋转），
  与标题同一水平线、紧贴右内边距；不占位、不挤压标题（标题 min-width:0 + 省略）。
- 底部工具行：13px / `--text-secondary`，行高 32px。

#### 工作区与面板容器 `layouts/Workspace.vue` / `components/panels/PanelContainer.vue` 🟢

- 面板默认 380px（宽档 `--chat-column-max` 的一半），分隔线 1px，拖拽时 100ms 无动画。
- 页签内容区底 = `--panel-bg`，与对话区同底。
- 无页签空态：居中图标 32px（`--text-tertiary`）+ 14px 说明，**不用 Arco `a-empty` 默认插画**（🟠，见第 6 章）。

### 3.2 对话区 `components/chat/`

#### 消息流与时间分隔 `ChatPanel.vue` / `ChatMessageItem.vue` 🟢

| 元素 | 规格 |
|---|---|
| 内容列 | `.chat-column`（最大 960px，见 1.4） |
| 日期分隔 | 居中、13px / `--text-tertiary`、上下留白 16px |
| 用户消息 | **右对齐气泡**：底 `--fill-1`（浅色浅灰 / 深色亮填充）、圆角 16px、内边距 10×14、正文 15px；连续多条时组内间距 4px |
| 助手消息 | **无容器**：直接落在画布上，正文 15px / 行高 1.65 / `--text-primary` |
| 消息间距 | 同一轮内 12px；跨轮 32px（用留白替代分隔线） |
| 消息悬浮 | 悬停消息块时，在该块**下方居中**淡入一个时间戳小胶囊：13px / `--text-primary`、`--fill-3` 底、`--radius-md`、内边距 2×8px，150ms 淡入（参考图出现过两次：`2:45` / `3:08`） |

助手正文虽无容器，但**段落间距要足**（段间 12px、列表项间 6px），这是"去容器化"仍然可读的前提。

#### 过程收纳头 `ChatTurnGroup.vue` 🟢（形态冻结）

- 一行 13px / `--text-tertiary`：`Worked for 25秒 ›`（完成）/ `Working for 1分钟 52秒`（进行中，**无 chevron**）。
- **下方一条 `--hairline` 分隔线**，贯穿内容列 —— 用来把"过程"与"答复"切开，**不要删**。
- 点击整行展开/折叠；hover 文字转 `--text-secondary`。
- 展开区结构以现有实现为准（🔒 只对齐令牌）。

#### 步骤轨迹与工具执行行 `ChatStepList.vue` / `ToolCallList.vue` 🟢（形态冻结）

- 工具执行行：**单行**，左侧 16px 方框图标（`--text-tertiary`）+ `--font-mono` 13px 命令摘要
  （`--text-tertiary`，**单行省略**，不折行）；进行中的行整行降到更淡一档（`--text-disabled` 量级）。
  行高 24px、与相邻正文间距 8px。
- 展开后的完整命令/输出：`--font-mono` 13px + `--fill-1` 底 + `--radius-md`、内边距 10×12px，
  **最大高度内滚**（不吸顶，`global.css:358-371` 有针对第三方库的同类修复，别回退）。
- 思考块折叠头：单行 13px / `--text-tertiary`（形如 `Thinking`），展开内容按正文排版、`--text-secondary`。
- 左侧缩进 16px 对齐，项间距 8px。
- 徽标（`DenialBadge.vue` / `SandboxBadge.vue`）：胶囊 `--radius-full`、高 20px、12px 文字、
  底为对应语义色 16% 混合、文字取语义色本身。
- **上述结构已冻结**（🔒 2026-10-15 拍板：过程展开态保持现有 UI），只做令牌对齐。

#### 正文（Markdown）→ **见第 4 章**

对话正文是全应用最高频的界面，有独立成章的完整规范（排版刻度、代码块、行内/块级元素、多宿主一致性）：
**`components/common/MarkdownBody.vue` 与 `components/markdown/*` 的改动一律先读第 4 章**。

#### 附件与引用胶囊（`AttachmentBar` / `ElementPill` / `QuotePill` / `SkillAttachment` / `OfficeAttachment`）🟡

- 胶囊：高 26px、`--radius-full`（图标+文字款）或 `--radius-md`（含缩略图）、底 `--fill-2`、文字 13px。
- 图片附件：48×48 缩略图、`--radius-md`、1px `--hairline`；右侧 12px 移除按钮 hover `--danger`。
- 悬停卡片：走材质卡（L3），宽 280–360px。
- 胶囊内文字超过 20 字符省略，`--text-secondary`；hover 变 `--text-primary`。

#### Composer 输入区 `ChatInput.vue` / `NewSessionForm.vue` 🟢

- 卡片：`--radius-2xl`(18px)、底 `--surface-bg`、`border: var(--surface-border)`、内边距 12–14px
  （浅色下就是"白卡 + 细描边"，深色下是"亮填充"）。
- 占位符：15px / `--text-tertiary`：「有什么可以帮你？」风格的一句话，不加问候语。
- 底部控制行（高 28px）从左到右：`+` 图标钮（28×28，hover `--fill-2`）→ 权限 chip → 弹性空白 → 模型/推理选择 → 发送键。
- **权限 chip**：`--radius-full`、13px、图标 14px；文字与图标同色（`--text-secondary`），
  **只有"完全放行"这类高危态**才用 `--warning`（参考实现里 `Ask for approval` 是纯中性、`Full access` 用琥珀色图标）。
  hover `--fill-2`。**窄宽度下可收成纯图标**（只留手形图标、无文字），此时必须有 tooltip 说明当前权限。
- **模型选择**：13px / `--text-secondary` + chevron 12px，透明底，hover `--text-primary`。
- **发送键 / 停止键**：32×32 圆形，**反色单色，不是强调色** ——
  可用 = `--text-primary` 实底 + `--bg-base` 符号（浅色下即"黑底白箭头"，深色下反过来）；
  生成中 = 同底色的**方块**（`--radius-md`，10×10px）；禁用 = `--fill-2` 底 + `--text-tertiary` 符号。
  切换用 150ms 交叉淡入。
- 附件条在输入卡**上方**独立成行（不挤在卡片内），间距 8px。
- 报错条与排队条（`PendingQueue.vue`）：复用材质卡 L3，13px，语义色图标 + 中性文字。

#### 交付物卡 `TurnArtifacts.vue` 🟠

- 一组横向卡片：宽 200–260px、底 `--surface-bg` + `--surface-border`、`--radius-lg`、内边距 12px；
  标题 15px/500 单行省略、副标题 13px `--text-tertiary`。
- hover 转 `--surface-hover`；点击开页签。整组上方一行小标题 13px `--text-tertiary`。
- **变更统计徽标**（参考实现 `3 files changed +98 -36` 🟢）：
  外框 = `--radius-full` + `--surface-bg` + `--surface-border`、高 24px、内边距 0 10px、13px；
  文案 `N files changed` 用 `--text-secondary`；新增数 `+98` 用 `--success`、删除数 `-36` 用 `--danger`，
  两组数字用 `--font-mono` 且字重 500；数字前的 `+`/`-` 不省略。

#### 审批卡 `ApprovalBar.vue` 🔒形态冻结 🟢

**结构以现有实现为准**（毛玻璃浮层 + 分类色图标块 + 等宽命令块 + 三个按钮），
2026-10-15 拍板：**不重画**，只做令牌对齐：

- 颜色：毛玻璃/底色/边框/阴影 → 第 2 章 L3 材质卡；分类色 → `--warning` / `--danger` / `--success`。
- 字体：命令与路径块 `--font-mono`；标题 15px/600、说明 14px/500、正文 13px。
- 尺寸：圆角 → `--radius-xl`；图标块 32×32 `--radius-md`；按钮高 28px `--radius-md`；内边距 16px。
- 按钮语义：拒绝 / 本会话允许 / 允许 = 次要填充 / 次要填充 / 强调色实底白字（第 3.4 节）。

#### 中断表单 `UserInputForm.vue` / `ChatFormBar.vue` / `MailComposeBar.vue` 🔒形态冻结 🟢

同上：**结构不变**（含"内嵌替代输入框"的形态、单字段单页、多步导航），只做令牌对齐：

- 字体：字段标题 15px/500、说明 13px、输入 14px；必填标记用 `--danger` 的 `*`（不写"必填"二字）。
- 尺寸：控件高 32px、`--radius-md`、内边距 16px、步骤圆点 6px。
- 颜色：输入控件走 `--surface-*`；提交按钮强调色实底；步骤点当前 `--accent-text`、其余 `--fill-3`。

### 3.3 浮层

#### 右键菜单 / 下拉菜单 🟢

- 材质卡 L3，`--radius-lg`(12px)，内边距 6px，最小宽 200px。
  **浅色下是"白底 + 1px 描边 + 软阴影"**（参考实现里明确可见描边），深色下是"半透明 + 毛玻璃"。
- 菜单项：高 28px（`--menu-item-height`）、文字行高 1.2（`--line-height-ui`）、`--radius-sm`、左侧图标 16px（`--text-tertiary`）+ 14px 文字 + 右侧快捷键提示（13px `--text-tertiary`）。
- hover/键盘选中：`--fill-2`（浅色下即浅灰）；禁用：`--text-disabled`（不降透明度）。
- 分组用 1px `--hairline` 分隔 + 上下 6px；**子菜单向右偏移 2px 叠加，贴近屏幕右缘时整体向左翻转**。
- 选中项（单选式）：右侧 ✓（14px，`--accent-text`）。

#### 气泡卡 / 悬浮卡 `HoverCard.vue` / `ElementPill` / `QuotePill` / `SkillAttachment` 🟢

- 材质卡 L3，`--radius-lg`，内边距 12×14px，最大宽 360px。
- 标题 13px/600 + 正文 13px/1.5 `--text-secondary`；卡内元信息 12px `--text-tertiary`。
- Arco Tooltip/Popover 已全局改写为同款（`global.css:446-497`），**不要再单独覆写**。

#### 命令面板 / 搜索浮层 `components/search/SearchModal.vue` 🟢

- 遮罩 `rgba(0,0,0,.28)`（深色 `.45`）+ 面板宽 720px、顶部偏移 88px、居中。
- 面板：材质卡 L3 加强版 —— `--bg-base` 96% + `--material-blur` + `--surface-border` + `--shadow-lg` + `--radius-xl`。
- 输入行：无边框，`--font-sans` 16px，占位符 `--text-tertiary`，内边距 16×18px，下方 `--hairline` 分隔。
- 分区标签：13px / `--text-tertiary`，内边距上下 8px。
- 行：高 32px、`--radius-sm`、hover/选中 `--fill-2`；左侧图标 16px；右侧元信息（项目名 13px `--text-tertiary`）与快捷键提示。
- 结果高亮：匹配片段用 `--accent-text` + 500 字重（不用黄底）。
- 空结果：居中 13px `--text-tertiary`，不放大插画。

#### 链接预览卡（外链 hover）🟢（2026-10-15 锚定）

鼠标停在正文的**外链**上时弹出（`🖱️ hover 约 300ms 后`；键盘聚焦与触摸不弹，触摸改用点击直达）。

- 位置：默认在链接**上方**、水平与链接居中对齐；贴顶时翻到下方，贴边时夹紧在视口内。
- 尺寸：宽 340–420px、最大高 240px；内容超出时摘要截断（不滚动）。
- 结构（三段，只有第 1、2 段之间有一条线）：
  1. **头部行**：地球图标 16px（`--text-tertiary`）+ **域名** 13px / `--text-secondary`；
     右端一个 `Open ↗` 动作（13px / `--text-primary` + 外链图标 14px，hover `--accent-text`）。
  2. 1px `--hairline` 分隔。
  3. **内容区**：标题 15px / 600 / `--text-primary`，最多 2 行省略；
     摘要 13px / `--text-secondary` / 行高 1.5，最多 2 行、末尾用省略号（`…`）；标题与摘要间距 6px。
- 材质：L3 材质卡（`--bg-1` 96% + 毛玻璃 + `--hairline` + `--shadow-md` + `--radius-xl`），内边距 16px。
- 交互：卡片自身可 hover（移入不消失），点击卡片 = 打开链接；`Esc` 关闭。
- **降级**：预览数据未就绪时，只显示头部行 + 一行骨架（`--fill-1`，高 12px）；抓取失败时头部行 + 13px
  `--text-tertiary` 的"无法预览"，**不显示空卡**。
- 与 3.3「气泡卡」的区别：气泡卡是**应用内对象**的说明（文件/元素/引用），本卡是**外部网页**的预览，不要混用。

#### 确认卡 `components/common/MacConfirm.vue` 🟢

- 宽 320–380px、材质卡 L4、`--radius-xl`；图标 32px 语义色 + 标题 15px/600 + 正文 13px `--text-secondary`。
- 按钮右对齐：取消（次级填充）/ 确认（强调色实底；破坏性动作用 `--danger` 实底），28px / `--radius-md`。
- 只允许一个主按钮；文案是动词短语（"删除会话"）。

#### 弹窗 `components/common/mac-modal.css` 覆盖的 `<a-modal>` 🟢

- 骨架已在 `mac-modal.css` 定义：`--radius-xl` + 96% 底 + 毛玻璃 + 发丝边 + `--shadow-lg`；
  浅色下把底改成 `--bg-base` 96% + `--surface-border`（保持可读）。
- 标题栏：13px/600 左对齐、内边距 13×16px、底 `--bg-2` 72%、下 `--hairline`。
- 底部：内边距 10×16px、上 `--hairline`、按钮高 28px / `--radius-md` / 13px。
- **两个变体，别再自造**（2026-10-15 收敛：`.fm-modal` / `.agent-modal` / `.scheduled-modal` 三套自写骨架已并入，
  三个文件共删约 88 行）：
  - `mac-modal`（**默认**）：半透明 + 毛玻璃，用于提示/展示类；
  - `mac-modal mac-modal--solid`：把玻璃底换成 `--panel-bg` 实底，用于**内容型弹窗**（知识库新建/编辑、
    智能体表单、定时任务表单、技能表单）。
  两变体共用同一套骨架（圆角 14 / 标题栏 / 发丝线 / 关闭热区 / 底部按钮形态），差异只有底色。
- **标题左对齐要改两处**：Arco 的 `.arco-modal-header` 与 `.arco-modal-title` **都是 flex 容器且默认居中**，
  只写 `text-align: left` 压不住 —— 必须同时给这两处 `justify-content: flex-start`（mac-modal.css 已内置）。
  排查这类"改了没反应"先看 computed 的 `justify-content`，别盯 `text-align`。

#### 大标题对话框（sheet 型）🟢（2026-10-15 锚定）

参考实现里「导入浏览器数据」「清理浏览数据」这类**任务型对话框**与 `mac-modal` 骨架是两个物种：
它没有毛玻璃标题栏、没有分区线，而是「大标题 + 说明 + 字段 + 选项组 + 底部按钮」的一页式表单。规范如下：

- 面板：宽 **720px**、`--radius-xl`(16px)、`--bg-1` 96% + `--material-blur` + `--hairline` + `--shadow-lg`；
  窗口居中（不是页签居中）；遮罩沿用 L4（`rgba(0,0,0,.28)` / 深色 `.45`）。
- 标题：**22px / 600 / `--text-primary`**，左上、页内边距 24px 起；右上角关闭 ×（16px，`--text-tertiary`）。
- 说明行：15px / `--text-secondary`，紧跟标题下方 8px。
- **字段行**：左侧 label 15px / `--text-secondary`（固定宽，右对齐或左对齐均可），右侧控件占满剩余宽度。
  参考实现里 `From` 这一行的控件是**选中态的选择器**（`--fill-2` 底 + 左图标 + 主文 + 灰副文，无描边）。
- **提示句**（如"导入前请完全退出 Chrome"）：15px / `--text-secondary` 的**纯文本**，
  **不要**做成 warning 色的 banner、不要加图标 —— 参考实现里它和普通说明同等重量。
- **选项组**（本节的核心控件，见 3.4「选项组 / 多选列表行」）：卡片 + 行分隔 + 行右侧控件。
- 底部按钮：右对齐、间距 8px、**高 36px**（比 `mac-modal` 的 28px 大一档，因为它是页面级对话）、
  `--radius-md`、内边距 0 18px、15px / 500。**主按钮用强调色实底白字**（`--accent-text`；
  2026-10-15 裁决：**不跟参考图的反色单色**），次要按钮用 `--fill-2`。破坏性动作（如 `Delete data`）
  用 `--danger` 实底 —— 参考实现里它不染红，本项目选择更保守的红色。
- 与 `mac-modal` 的分工：**表单/内容弹窗**（新建、编辑、设置类）用 `mac-modal`；
  **任务型对话框**（导入、清理、批量操作确认）用本节的大标题形态。

#### HUD 提示 `Message` 🟢

- 已全局改写为材质卡（`global.css:373-444`）：`--radius-xl`、`--bg-2` 88% + 毛玻璃、`--shadow-md`、13px/500 内容。
- 顶部让开标题栏（`top: 52px`）。**不用** `Notification`。

#### 选项缩略图选择器 🟢（参考"外观 → 模式"那四个小图）

- 缩略图 60×40px、`--radius-md`、1px `--surface-border`；缩略图内容用真实的浅/深配色小样。
- 选中：外圈 2px `--accent-text` + 距缩略图 1px 间隙（用 `box-shadow: 0 0 0 1px var(--bg-base), 0 0 0 3px var(--accent-text)` 实现，不占布局）。
- hover：`--surface-hover`；点击即生效，不需要确认按钮。
- 同一容器里最多 4 个，横向排列、间距 8px。

### 3.4 表单控件

Arco 控件的**统一压平**已在 `styles/inputs.css`（全应用）与 `styles/settings.css`（设置页作用域）里完成。
新控件按下列规格，改完检查两处是否仍然生效。

#### 输入框 / 文本域 / 下拉 / 数字 / 输入标签（输入/选择类）🟢

- 容器：底 `--surface-bg`、`border: var(--surface-border)`、`--radius-md`、高 32px（文本域最小 72px）、
  内边距 0 12px。
  > **浅色下是"白底 + 细描边"、深色下是"亮填充 + transparent 边框"** —— 这正是规则 2 与规则 3 的落点，
  > 也是与现状（浅色下灰填充、无描边）差别最大的地方。
- hover：`--surface-hover`。聚焦：底 `--bg-base` + 光环 `0 0 0 3px color-mix(accent 22%)`。
- 校验错误：光环换 `--danger` 20%（**不换描边、不抖**）。
- 禁用：保持面、文字 `--text-disabled`、`opacity: 1`。
- 占位符：14px / `--text-tertiary`。

#### 按钮（动作类）🟢

| 类型 | 底 | 文字 | 用法 |
|---|---|---|---|
| 主要 | `--accent-text` 实底 | `#fff` | 每屏一个主动作 |
| 次要/默认 | `--fill-1`，hover `--fill-2` | `--text-primary` | 普通动作（**纯填充、无描边**） |
| 文字 | 透明，hover `--fill-1` | `--text-secondary` | 行内动作 |
| 危险 | `--danger` 12% 底 | `--danger` | 破坏性（次级位） |
| 危险主要 | `--danger` 实底 | `#fff` | 破坏性的最终确认 |

- 高度：28px（弹窗/卡片内）或 32px（页面内）；`--radius-md`；13px / 500；内边距 0 14px。
- 按压 `scale(0.97)` 100ms；禁用 `--fill-1` + `--text-disabled`。
- **按钮不带描边**（与输入/选择类相反）；`--shadow-sm` 只留给 mac-modal 主按钮这类"贴纸感"按钮。
- **主按钮一律用强调色实底白字**（2026-10-15 裁决）：**不要**跟随参考实现把弹窗主按钮做成反色单色。
  **反色单色只保留给发送键 / 停止键**（3.2），其余场景一律 `--accent-text`。

#### 开关 🟢

- 尺寸 40×22px、`--radius-full`；手柄 18px 白 + `--shadow-sm`。
- 关：`--fill-3`；开：**`--accent-text`**（参考实现浅深两色下都是蓝色）。
  > **裁决**：仓内现有两套并存（`settings.css:106` macOS 绿、`global.css:315` 灰），统一为强调色。
- 无文字标签，标签在左侧行内。

#### 复选 / 单选 / 分段控件 / 选项组 🟢

**复选框**（🟢 锚定）：18–20px、`--radius-sm`(6px)；选中 = `--accent-text` 实底 + 白勾；
未选 = 透明底 + 1.5px `--border-strong`，hover 时描边转 `--text-tertiary`。多选列表里右对齐（见下）。

**单选圆点**：同尺寸、同语义（🟠 推导）。

**分段控件**（🟢 锚定，参考"清理浏览数据"的时间范围那排；共享件 `components/common/SegmentedControl.vue`）：
- 一排 `--radius-md` 胶囊，容器**不套槽**（无底色、无内边距），项间距 8px。
- 尺寸：高 34px、内边距 0 14px、15px / 500。菜单内用 `size="sm"` 紧凑档：高 26px、内边距 0 10px、13px。
- 未选：透明底 + `--text-secondary`，hover `--fill-1`；**选中：`--fill-3` + `--text-primary`**。
- 同排最多 5 项，再多改用下拉；不换行。

**选项组（多选/设置列表）**（🟢 锚定，参考"清理浏览数据"那张六行列表）：
- 容器：`--radius-lg` + **1px `--hairline` 描边**（组的边界靠描边，不靠底色差）+ 无投影。
- 行：高 68px（双行文本）/ 56px（单行），内边距 16px；行间 `--hairline` 且**左右内缩**。
- 行内布局：图标 20px（`--text-tertiary`）+ 标题 15px/500 `--text-primary` + 说明 14px / `--text-secondary`
  （行高 1.4，可两行）+ **右侧控件**（复选框 / 单选圆点 / 开关）。
- **开关 vs 复选框的用法**：**二值开关**（导入哪些类别）用开关；**可多选的清单**（清理哪些数据）用复选框；
  同一个组里不要混用两种。
- 点击整行即切换该项（不只点控件）。
- 数量为 0 时说明文本照常显示（如"Cookies currently stored for no sites"），**不置灰、不禁用**。

#### 分页 / 进度 / 标签 / 徽标 🟠

- 分页：按钮 28×28、`--radius-md`、13px；当前页 `--fill-3` + `--text-primary`。
- 进度条：高 4px、`--radius-full`、轨道 `--fill-2`、填充 `--accent-text`；不确定态用 `--text-tertiary` 滑动。
- 标签 `a-tag`：高 20px、`--radius-full`、12px；中性 = `--fill-2` + `--text-secondary`；语义 = 语义色 16% 底 + 语义色文字。
- 计数徽标：12px、`--fill-3` 底、`--radius-full`、内边距 1×6px。

### 3.5 数据展示

#### 文件树 `components/sidebar/FileTreeNode.vue` / `FileExplorerPanel.vue` / `panels/WorkspaceFilesPanel.vue` 🟢

- 过滤框：高 28px、`--surface-bg` + `--surface-border`、`--radius-md`、左侧放大镜 14px `--text-tertiary`。
- 行：高 28px、`--radius-md`、缩进 12px/层；chevron 12px（展开时旋转 90°）、名称 13px、`--text-primary`。
- hover `--fill-1`；选中 `--fill-2`；不做行内分隔线。
- 文件夹图标 `--ficon-folder`，文件图标按类型（唯一多色位）。
- 空目录/未选中：13px `--text-tertiary`。

#### 表格 🟠

> 参考实现没有表格（设置页是"卡片 + 行"）。按既有面语言推导：

- 表头：13px / 500 / `--text-tertiary`，底 `--fill-1`，行高 32px。
- 行：高 40px（单行）/ 56px（双行），分隔用 `--hairline`，hover `--fill-1`，选中 `--fill-2`。
- 单元格内边距 8×12px、13px；数字列右对齐且用 `--font-mono`。
- 排序指示：chevron 10px，`--text-tertiary`（激活 `--text-primary`）。
- 分页见 3.4。

#### 列表 / 卡片列表（`AgentCard` / `SkillCard` / `PluginCard` / `Card` 类）🟢

- 底 `--surface-bg` + `border: var(--surface-border)`、`--radius-lg`、内边距 12–16px。
- hover `--surface-hover`（浅色白→浅灰 / 深色亮一档）；选中 `color-mix(accent 15%)`。
- 标题 15px/500 + 描述 13px `--text-secondary` + 元信息 13px `--text-tertiary`。
- 统一由 `styles/cards.css` 提供（新卡片类名加进那份选择器表，别各写一份）。

#### 代码与 diff 🟢

- `--font-mono`、13px、行高 1.7；**行号 `--text-tertiary` 右对齐**，与代码留 16px 沟（行号只出现在文件预览，
  正文代码块没有行号，见 4.3）。
- 底色 `--fill-1` 区块 + `--radius-lg`；不做行悬停高亮带。
- **面包屑条**（文件预览顶部）：整条装在一个 `--fill-1` 的**凹陷面**里（`--radius-md`、内边距 5×12px、无描边），
  路径段 13px / `--text-secondary`，分隔符 `›` 用 `--text-tertiary`，**当前文件名 13px/600 + `--text-primary`**。
- diff：新增行 `--success` 12% 底、删除行 `--danger` 12% 底，行号列同色加深到 20%；不染文字色。
- 变更统计（`+98 -36`）见 3.2 交付物卡的徽标规格。
- **语法着色：文件预览用完整主题**（关键字红橙、导入的模块名橙棕、类型与函数名蓝青、类关键字紫、注释灰斜体），
  与正文代码块的**三色极简配色是两套**（见 4.3），**不要合并**。

#### 终端 `panels/TerminalPanel.vue` 🟢（2026-10-15 锚定）

- **背景与前景跟随应用主题**（2026-10-15 用户确认）：底 `--bg-base`、文字 `--text-primary`，明暗切换时一起变。
  **不要**固定深色底，也不要为背景另造 `--terminal-*` 令牌。
  （参考图里浅色窗口下终端呈深色，是因为当时选了深色系的终端配色预设，不是"终端永远深色"。）
- ANSI 16 色仍由 `theme/terminal.ts` 的配色预设决定（设置页可选）；选深色预设时浅色应用下也会是深底
  ——**那是用户选择的结果，不是默认行为**。
- 文字 `--font-mono` 14–15px / 行高 1.5。
- **无 chrome**：没有工具条、没有面板头、没有状态行——终端内容直接铺满页签。
- 光标为**块状**（占满一个字符宽、慢闪或不闪）。
- 内边距很小（约 8px 上下 / 12px 左右），文字贴边是正常的；不要为了"好看"补 16px 内边距。
- `ls` 等多列输出由 shell 决定，**UI 不做列处理**。

#### 内置浏览器 `panels/BrowserPanel.vue` 🟢（2026-10-15 锚定）

**页签**（在窗口页签条里，与其它页签同规格）：前置 16px 类型图标（网页用地球/文件用 `</>`）+ 标题，
激活页签有 ×；标题文本 `Pelican Pedal Club · 鹦鹉骑自行车` —— 站点名与页面名用 ` · ` 连接。

**浏览器工具条**（页签条下方一行，高 32px；2026-10-20 按参考图重锚定，后按用户要求收紧两档）：
- 与网页内容同底（不铺 `--surface-*`），**无下边框**，工具条与内容之间只靠留白区分；
  页签条与工具条之间保留面板上边界的那条线。
- 左：**导航胶囊** —— 一个 `--radius-full` 胶囊装 `←` `→` `|` `↻` 三个 24×24 图标钮，
  胶囊高 28px、内边距 0 3px（两侧留一点空隙，按钮不贴胶囊边）、底 `--bg-2` + 1px `--border-base` 描边；
  后退/前进与刷新之间是 1px×14px 竖分隔线。图标 `--text-secondary`，hover `--fill-2`、激活 `--fill-3`
  （按钮高亮是**正圆**，`--radius-full`）；不可用时 `opacity: .4`（**不隐藏**，保持位置稳定）。
- 中：**地址栏是一颗更亮一档的胶囊**（输入控件语言，非“常态纯文本”）：高 28px、`--radius-full`、
  底 `--bg-3` + 1px `--border-strong` 描边、左右内边距 10px、文字 13px `--text-primary`。
  **空态提示居中**（`--text-tertiary`），光标仍在输入流左侧 —— 提示是绝对定位的覆盖层，不是原生 placeholder。
- 右：**操作胶囊**，与导航胶囊同规格，装拾取元素、截图、录像、`⋯`（更多）四个图标钮。
- 胶囊之间的水平间距 8px；工具条贴近上方页签条（`.bw-wrap` 顶部 0 + 工具条内 2px），不额外留白。
- **画布四角锚点**（仅自定义视口）：**可见**网页画板四角各一颗 12px 圆形手柄（圆心落在角上、
  底 `--bg-base` + 1.5px 半透明描边 + 小投影；hover / 拖拽中描边转 `--accent-text`，
  拖拽中填充强调色；画板比面板宽时贴住可见边缘，不跑到屏幕外，拖拽中实时跟随可见四角）；
  拖拽沿对角线等比缩放，**只改缩放百分比、不改视口宽高**，范围 25%–200%，
  拖拽中在画板顶部居中显示当前 `63%` 浮标。铺满（adaptive）模式不显示锚点。
- **视口/缩放不占独立行**：铺满/自定义、宽高、预设尺寸、缩放全部收进 `⋯` 菜单（2026-10-20 用户裁决），
  工具条永远只有这一行，不出现第二条控件行。
- **激活态用组内填充**（`--fill-3` + `--text-primary`），不用强调色实底 —— 强调色实底胶囊的早期参考约定
  在本轮参考图中不再出现，浏览器 chrome 统一走灰阶填充。
- 页面遮罩/元素高亮由主进程绘制，不在规范内。

**`⋯` 菜单**（🟢 锚定）—— 它是**另一族菜单**，与应用动作菜单（3.3）不同：
- 宽 264px、行高 28–30px、文字 13px、左对齐；**项不带左侧图标**，纯文字。
- 顶部是**设置区**（不是动作项）：
  - 小标题「视口」12px `--text-tertiary`。
  - 视口模式用共享分段控件（`size="sm"`：26px / 13px，不套槽、选中 `--fill-3`，见 3.4）。
  - 自定义模式下方跟 `宽 × 高` 两个数字输入（26px、无边框、`--fill-1` 底），再跟**两列预设格**
    （桌面 1366 / 1280 / 1024、笔记本 900、平板 768、手机 375，26px 高）。
  - 「页面缩放」行右侧是一组 `−  50%  +`，装在**1px `--border-base` 描边的胶囊容器**里
    （26px 高、26px + 48px + 26px 三格、格间 1px 分隔）；两端档位禁用，不做滑杆。
    缩放存的是**精确百分比**（四角拖拽可得到任意整数，如 63%）；− / + 落到相邻档位
    （50 / 67 / 75 / 80 / 90 / 100 / 110 / 125 / 150 / 200），越过端点夹在 25% / 200%。
- 设置区与动作区之间用 1px `--border-light` 分隔；动作项为 `DevTools` / 复制链接 / 外部打开，
  点击后菜单收起；设置控件点击不收起（`@click.stop`）。
- 文案用动词/名词短语（`Open DevTools` / `Copy link`）；
  **只有会再弹一步的项才带 `...`**（`Import cookies and passwords...` 会开对话框，`Downloads` 不会）。

### 3.6 页面级

#### 设置页 `components/settings/SettingsView.vue` / `SettingsMenu.vue` / `pages/*` 🟢

安全设置的文件/网络/危险命令名单不内嵌展示或编辑，使用 `SettingsConfigFileRow` 显示实际 `security.json` 路径；「更改」把设置页切换为**页内 JSON 编辑器**（不弹窗），「打开配置文件」交给系统默认应用；界面保存仅更新可见控件，保留文件中的名单。

2026-10-20 参考图更新：设置分组全部展开，取消折叠箭头和点击收起行为；标题在圆角边框外左上方，组内行以细线分隔。共享 `SettingsSection` 承担结构，`styles/settings.css` 统一布局与明暗主题。

- 左栏宽 220–240px，底为玻璃层（见 3.1）；项高 32px、`--radius-md`；
  **激活 = `--fill-2`**（参考实现浅深两色下都是灰底填充，不是蓝环），文字 `--text-primary`。
- 搜索框：高 34px、`--radius-full`、底 `--fill-1`（玻璃层上用填充而不是描边）、内边距 0 12px、放大镜 15px。
- 内容区：**统一内容列** —— 定宽 832px + 水平居中；顶部留白 24px、底部留白由页脚承担，由 `SettingsView.vue` 的
  `.settings-scroll` 一层包住（各页面不再各自控制留白）。这样左右留白永远相等、切换页面不跳；页面标题 28px/600，
  距顶部 24px、距首个分区 20px。
- 分区小标题（"Permissions" / "General"）：14px/500 / `--text-primary`，距卡片 10px；分区之间 24px。
- 卡片：底 `--surface-bg` + `border: var(--surface-border)` + `--radius-xl`(14px)、无投影。
- 行：最小高 56px（双行说明），上下内边距 12px；行间分隔 `--hairline` 且**两端都内缩**（从左右内边距处起画，不顶到卡片边）。
- 页脚：**不画分割线**；分组页整页滚动，页脚 sticky 在滚动容器底部（按钮常驻底部、不随内容滚走）。
- 行内左侧：标题 15px/500 + 说明 14px / `--text-secondary`（行高 1.45）；右侧控件右对齐、垂直居中。
- 行内文本链接（"Learn more"）：`--accent-text`，不加下划线。
- 设置分组不提供折叠头，所有设置内容始终可见。

#### 引导页 `onboarding/*` 🟢（独立约定）

- **已有既定视觉约定，不要套用本文的卡片规范**：无卡片/无面板/无要点清单，内容直接落在动态背景上；
  主按钮用 `--accent-text` 实心（§3.4）；步骤点可点且 `no-drag`。详见 `docs/cloud-server.md:151-157`。
- 偏好步只保留明暗 / 语言 / 保持电脑运行，**不提供主题色设置**（沿用当前主题预设）。
- 本文只约束它继承的令牌（字体、字号、动效、聚焦光环）。

邮箱与模型设置共用 `settings-editor-*` 样式：相同的页面留白、220px 左侧列表、底部添加按钮、24px 详情内边距、标题与操作栏。两页输入控件统一 medium，列表、字段标签与控件文字统一 13px，配置标题 14px。邮箱设置的新增与编辑共用同一表单：字段标签、账号列表、控件文字采用 13px（`--font-size-sm`），配置标题 14px，输入控件采用 medium 尺寸。按表单容器宽度响应：460px 以下主机独占一行、端口与 TLS 并列，300px 以下单列；设置内容区 600px 以下账号列表移至上方，避免挤压表单。

#### 视图页（看板 / 定时 / 邮件 / 知识库 / 记忆 / 用量）🟠

> 六者参考实现都没有对应物。统一按下列骨架，**不再逐个发明**：

- 页面骨架：顶部 44px 工具条（标题 15px/600 左对齐 + 右侧动作按钮）→ 内容区 24px 内边距。
- 悬浮预览（任务看板与共享 `HoverCard`）使用独立 `--hover-card-bg`：浅色白底，深色 `#343434`，箭头同色，投影走 `--hover-card-shadow`，确保与页面及静态卡片区分。
- 任务看板性能：列使用 inline-size containment；卡片使用 `content-visibility: auto` 与自动实测高度占位，屏外卡片跳过布局/绘制。预览 DOM 文本最多 2000 字符，原文与搜索仍读取完整会话。
- 任务看板卡片（`BoardCard`）复用 `styles/cards.css`：默认显示 `--fill-1` 背景，悬停/键盘聚焦加深至 `--fill-2`（2026-10-19 确认）。
- 列表/看板：卡片走 3.5「列表/卡片列表」；看板列标题 13px/500 `--text-tertiary`，列间距 16px。
- 三栏页（邮件）：左栏 240px 同设置左栏语言；中栏列表行高 64px、选中 `--fill-2`；右栏内容区底 `--bg-base`。
- 图表（用量统计）：坐标轴/网格线 `--fill-3`，文字 13px `--text-tertiary`；系列色只用「强调色 + 语义色 + 3 个中性灰」，
  禁止彩虹色板；图例 13px。
- 抽屉（记忆）：材质卡 L4 + 左侧 1px，宽 480px。

### 3.7 交互态（全组件通用）

| 态 | 表现 |
|---|---|
| hover | 深色：`--fill-1`→`--fill-2`；浅色：`--surface-bg`→`--surface-hover`。**不改尺寸、不加描边** |
| 选中/激活 | 列表行 `--fill-2`；(结构性)选项 2px `--accent-text` 环；强调底 `color-mix(accent 15%)` |
| 聚焦（键盘） | `outline: 2px solid color-mix(accent 45%)` + `outline-offset: 1px`，`:focus-visible` 才显示 |
| 聚焦（输入） | 3px 柔光环（见 3.4） |
| 禁用 | 文字 `--text-disabled`；容器保持面；**不整体降低 opacity** |
| 加载 | 行内 14px spinner（`--text-tertiary`）或行尾 12px 环形 spinner（见左栏）；区块用 3 行 `--fill-1` 骨架屏，不用整屏遮罩 |
| 空态 | 居中：图标 32px `--text-tertiary` + 主文案 15px `--text-secondary` + 可选动作按钮；垂直留白 48px |
| 错误 | 行内条：`--danger` 12% 底 + `--danger` 图标 + 13px `--text-primary` 文案 |

---

## 4. 会话正文（Markdown）渲染规范

> **为什么单独成章**：对话正文是本应用停留时间最长、出现次数最多的界面——一次间距或颜色的偏差会被每一轮对话放大。
> 参考实现（浅深同内容两张对照图）体现的是**极简渲染**：不给节点加容器、不画多余的线、颜色只用三类
> （正文色 / 语义灰 / 代码三色），层级靠字重与留白表达。
> 落地位置：`components/common/MarkdownBody.vue`（唯一正文渲染入口）+ `components/markdown/renderers.ts`（节点渲染器单源）。
> 文件预览 / 记忆页 / 会话轨迹**复用同一套**，不要另写一份。

### 4.1 五条极简原则

1. **不加容器**：段落、标题、列表、引用块都是"裸"文本——不加底色、不加边框、不加圆角。
   正文里唯一带面的块级元素是**代码块**（凹陷面，见 4.3）；表格**不套容器**。
2. **不画线**：不用水平线或色块划分区块；块与块之间只靠留白。唯一允许的线是 `---` 产生的分隔线（用户显式意图）
   与表格的行分隔。
3. **颜色收敛**：正文 `--text-primary`；次级信息（列表符号、行号、注释、占位）`--text-tertiary`；
   彩色只允许出现在链接/引用（`--accent-text`）与代码块内部（4.3 的三色）。
4. **层级靠字重与留白**：标题不换色、不加下划线、不加竖条，只放大字号并收紧字距。
5. **不放大装饰**：不渲染作者写的 HTML 样式；无 emoji 图钉、无插画；正文里的图标一律 16px 单色。

### 4.2 排版刻度 🟢

| 项 | 值 |
|---|---|
| 正文 | **14px**（`--font-size-md`）/ 行高 **1.5**（`--line-height-body`）/ `--text-primary`——与 UI 默认同档、行高比 UI 控件松 |
| 段落间距 | 12px（≈0.8 行；**不要**用空行式的 24px，正文会散） |
| 列表缩进 | 20px/级；列表项间距 6px |
| 块级元素（代码块/表格/引用）上下留白 | 16px |
| 连续块之间 | 不做额外压缩，保持 16px 均匀 |

标题层级（🟠 推导——参考图只出现了一级小标题）：

| 级别 | 字号 / 字重 | 段前 / 段后 |
|---|---|---|
| h1 | 20px / 600 / `-0.01em` | 24 / 8 |
| h2 | 17px / 600 | 20 / 6 |
| h3（参考图里 `R17：…` 这一级） | 16px / 600 | 16 / 6 |
| h4 及以下 | 15px / 600（与正文同号，只加粗） | 12 / 4 |

### 4.3 代码块 🟢（本章锚定最完整的一块）

**容器**：**凹陷面** —— 底 `--fill-1`（浅色下也是灰填充，**不是**白底 + 描边）、`--radius-xl`(14px)、
内边距 20×20px（参考图留白很大，别压到 12px）、**无边框、无投影、无内部分隔线**。

**头部**（与代码同底、无独立底色、**无下边框**）：
- 左侧：`</>` 图标与语言名，纯文字、无 chip 底色。
- 右侧：只保留**复制**图标钮；关闭折叠、下载与全屏。
- 工具条与代码内容使用相同的 `--fill-1` 底色；两者之间不画分隔线。
- **头部不吸顶**（`global.css:358-371` 已针对第三方库修过这个 bug，别回退）。

**正文**：`--font-mono` **12.5px**（`--font-size-code`）/ 行高 **1.6**（`--line-height-code`）——参考实现实测
字符步距 7.51 CSS px（÷ SF Mono 的 0.6em）= 12.5px、行距 20 CSS px；**无行号**（行号只出现在文件预览，见 3.5）；
超长块整体 `max-height` 内滚；不出现横向滚动条（必要时软换行）。

**极简配色** —— 只有 3 种着色，其余一律正文色：

| 元素 | 浅色 | 深色 |
|---|---|---|
| 关键字（`def` `class` `import` `from` `return` …） | 紫 `#9a5cd0` | 紫 `#c08ae8` |
| 字符串与文档串定界符（`"""`） | 绿 `#3fa34d` | 绿 `#6cc46f` |
| 注释 | `--text-tertiary` + 斜体 | 同 |
| 其他（标识符、括号、运算符、数字） | `--text-primary` | 同 |

> 与文件预览的语法配色**有意不同**：文件预览是完整主题（橙棕导入名、蓝青类型、红橙关键字…见图 4/5），
> 正文代码块刻意只留三色。**改配色时不要顺手把两者统一。**

### 4.4 行内元素

| 元素 | 规格 |
|---|---|
| 行内代码 `code` | `--font-mono` 0.875em、底 `--bg-2`、0.5px `--border-base` 描边、`--radius-sm`、内边距 0 5px；`inline-flex` 垂直居中 |
| 文件引用（`` `a.py:47` ``） | **纯链接文字，无底色/描边**；文件图标在文字外（1.1em + 5px 间距，与文字中线对齐）；文字用 `--link-text`（同链接），行号 70% 不透明度；hover 加点状下划线，`title` 显示完整绝对路径；点击开文件页签并定位到行 |
| 链接 | `--link-text`、**任何状态都不加下划线**（2026-10-15 确认）；hover 只做颜色微调（深色提亮 / 浅色加深约 8%） |
| 外链（正文里给出的网址/域名） | 前置**地球图标 14px** + 域名文本，同为 `--accent-text`、**无底色、无下划线**；hover 弹**链接预览卡**（见 3.3）。行内出现裸 URL 时也按此渲染，**不要把长 URL 原样铺出来** |
| 网页引用角标 `[n](url)` | `WebRefBadge`（2026-10-19 参考图锚定）：12px 等宽编号、`--text-secondary` 文字 + `--fill-2` 灰底、圆形无边框（`--radius-full`），高/最小宽 20px、水平内边距 5px，多位编号自然加宽为胶囊，与正文基线对齐；hover 为 `--fill-3` + `--text-primary`，键盘聚焦有可见轮廓。保留来源 tooltip 与应用内打开行为 |
| 加粗 / 斜体 | 600 / 斜体，**不换色**；强调色只留给链接与引用 |
| 提及 chip | 复用 `MentionText.vue`，不要在正文里重写 |
| 删除线 | `--text-tertiary` + 删除线；`==高亮==` **不渲染**（按纯文本显示，无底色标记） |

### 4.5 块级元素（🟡 部分锚定：无序列表 / 表格已锚定）

- **无序列表**（🟢 锚定）：`--text-tertiary` 的圆点（约 5px）、缩进 20px；**折行采用悬挂缩进**
  ——第二行对齐到文字起始位置，不回到圆点下方；**项间距大于行距**（约 8px），让每条目可辨；
  列表整体上下留白同段落（12px）。
- **有序列表**（🟠）：`--text-tertiary` 数字右对齐（不用等宽）；
  嵌套再缩进 20px 并换符号（实心圆 → 空心圆 → 方块）；列表项内含多段时行距收一档。
- **任务列表**：16px 复选框（同 3.4 复选规范）；勾选项文字转 `--text-tertiary` + 删除线（**不染绿、不加底色**）。
- **引用块**：左侧 2px `--border-strong` 竖线 + 左内边距 12px + `--text-secondary`；**不给整块底色**。
- **表格**（🟢 锚定）：
  - 裸表格：不套容器、无边框、无圆角、无斑马纹、无行 hover 高亮；与正文**左对齐**（无额外缩进）、上下留白 16px。
  - 表头：**无底色**（不要 `--fill-1`）、文字 `--text-secondary`、与正文**同字号**、字重 500；
    表头下方一条 `--border-base`（比行分隔线明显一档，用来划出表头）。
  - 行分隔：`--hairline`，**贯穿整表宽度**（不内缩）。
  - 单元格：垂直内边距 16px、列间距 32px、内容**顶部对齐**、字号与正文一致。
  - **所有列（含数值列）一律左对齐**；数字用**正文字体**（不换等宽）、保留千分位 —— 参考图没有做数字右对齐。
  - 单元格里可以放链接与行内代码，按 4.4 的规则渲染。
  - 列多/窄窗时**容器横向滚动**，不要压缩字号、不要隐藏列。
- **分隔线**：1px `--hairline`，上下留白 16px。
- **图片**：最大宽度 100%、`--radius-lg`、居中；加载失败显示 13px `--text-tertiary` 占位（不显示破图图标）。
- **内联 HTML**：不渲染作者样式（防注入），按纯文本显示。
  `[data-stream-markdown='html-previewer']` 的预览 iframe 固定为亮色纸面（`global.css:344-356`），别改。

### 4.6 Frontmatter / 元数据块

`MarkdownFrontmatterHeader.vue`：`key: value` 堆叠或两列，键 13px `--text-tertiary`、值 13px `--text-primary`；
装在一个 `--fill-1` 的**凹陷面** + `--radius-md` 里；**不参与正文的段间距计算**（上下各留 12px）。

### 4.7 多宿主一致性

正文渲染器被对话正文、文件预览、记忆页、会话轨迹共用（`components/markdown/renderers.ts` 是单源）。
**差异只允许有两处**：基准目录注入（`FILE_REF_BASE`）与是否显示行号；其余样式一律共用，不要在某个宿主里
单独覆写 markdown 样式。

### 4.8 本章仍缺的参考

一张**含「有序列表 / 引用块 / 任务列表」**的正文截图（浅深任一即可）。

- **已锚定**：小标题、段落、无序列表、代码块、行内代码、文件引用、外链 + 其 hover 预览卡、表格、消息时间戳。
- **仍为推导**：有序列表、引用块、任务列表、分隔线、图片、frontmatter 排版。

## 5. 覆盖矩阵

「状态」= 本文对该组件的规范来源：🟢 参考图锚定 / 🟡 部分 / 🟠 推导待参考 / 🔒 形态冻结。
「现有实现」列指规范与当前代码是否一致（✅ 基本一致 / ⚠️ 有明确差异，见第 7 章）。

| 分类 | 组件 / 页面（数量） | 章节 | 状态 | 现有实现 |
|---|---|---|---|---|
| 外壳 | StatusBar、PanelTabBar、PanelContainer、PanelSplitter、Workspace、AppLayout、**分裂按钮（新）**、**侧栏实色（修订）** | 3.1 | 🟢/🔒 | ⚠️ 一项待实现 |
| 左栏 | AppSidebar（+运行 spinner、固定实色）、FileExplorerPanel、FileTreeNode | 3.1 / 3.5 | 🟢 | ⚠️ 填充档 |
| 对话 | ChatPanel、ChatMessageItem、ChatMessageRail、TurnStatusBar、TurnArtifacts、DenialBadge、SandboxBadge、HoverCard、FooterIconButton、SessionInfoCard、SessionGoalButton、TaskPlanButtons、ChildSessionsList、ChatFormResume | 3.2 | 🟢 | ⚠️ 气泡底色 |
| 过程 | ChatTurnGroup、ChatStepList、ToolCallList、ThinkCard | 3.2 | 🔒🟢 | ✅ 形态保留，令牌待对齐 |
| 中断件 | ApprovalBar、UserInputForm、ChatFormBar、MailComposeBar | 3.2 | 🔒🟢 | ✅ 形态保留，令牌待对齐 |
| 正文 | MarkdownBody、CodeBlock、MarkdownFrontmatterHeader、FileRefChip/Code、LinkRenderer、WebRefBadge、MemoryConceptLink | **4** | 🟢/🟠 | ⚠️ 代码块头部与配色 |
| 输入 | ChatInput、NewSessionForm、ModelEffortSelect、FileBranchBar、PendingQueue、AttachmentBar、ElementPill/Attachment、QuotePill/Attachment、SkillAttachment、OfficeAttachment、MentionInput/Popover/Text | 3.2 / 3.3 | 🟢 | ⚠️ 发送键/面语言 |
| 浮层 | SearchModal、MacConfirm、mac-modal 弹窗、**大标题对话框（新，任务型）**、**链接预览卡（新）**、Arco 菜单/下拉/Tooltip/Popover、Message HUD、选项缩略图选择器（新） | 3.3 | 🟢 | ⚠️ 浅色描边、缺大标题与预览卡 |
| 表单控件 | 输入/文本域/下拉/数字/tag/按钮/开关/复选/**分段控件**/**选项组**/分页/进度/标签 | 3.4 | 🟢/🟠 | ⚠️ 面语言、开关、主按钮 |
| 面板 | FilePreviewPanel、CodeReviewPanel、CodeDiffView、WorkspaceFilesPanel、SessionTracePanel、SessionPanel、PendingSubSessionPanel、SourceResultsPanel、MemoryResultsPanel、MemoryConceptPanel、MemoryRawPanel、PluginSitePanel、**BrowserPanel（已锚定）**、**TerminalPanel（已锚定）** | 3.5 | 🟢/🟠 | ⚠️ 工具条底色/地址栏形态 |
| 预览 | OfficePreview、OfficeViewerRenderer、KbMarkdownPreview、FilePreviewModal、DirectoryPickerModal | 3.5 | 🟠 | ⚠️ 待核 |
| 设置 | SettingsView（实色）、SettingsMenu、registry、17 个设置页 | 3.6 | 🟢 | ⚠️ 卡片/开关/按钮 |
| 引导 | OnboardingApp、StepWelcome/Choice/Model/Prefs、WindowControls | 3.6 | 🟢(独立) | ✅ 既定约定 |
| 视图 | ChatView、NewChatView、ScheduledView、TaskBoardView、KnowledgeView、MemoryView、MailView、IconsView | 3.6 | 🟠 | ⚠️ 待收敛 |
| 其他 | AgentCard/FormModal、SkillCard/FormModal、PluginConfigForm、BoardCard、DeliverablesPanel、GitGraphCard、BranchMenu、mail/* 6 件 | 3.5 / 3.6 | 🟠 | ⚠️ 界面级表格/图表待定 |
| 图标 | UI 图标 108 字形 / 121 注册名、文件图标 64、品牌图标 84、遗留 `i-lucide-*` 1 处 | **1.6** | 🟢 | ✅ 受控目录已建（遗留 1 处待换） |
| 状态 | 空/载/错/禁/聚焦/hover/选中 | 3.7 | 🟡 | ⚠️ 空态用 Arco 默认 |

**未被覆盖的能力**（本规范暂不约束，出现时另议）：插件自带的页面样式（`plugins/*/style.css` 是独立世界）、
Office/PDF 预览内部的第三方渲染器样式、内嵌 iframe 内的第三方页面。

---

## 6. 已拍板的决策与待补参考

### 6.1 已拍板（2026-10-15）

| # | 决策 | 落点 |
|---|---|---|
| A | **侧栏用固定实色**（2026-10-20 修订原毛玻璃：全屏动画透色无法在快照前拦截） | 3.1 侧栏底色；`--sidebar-solid-bg`，非原生平台 `--bg-1` |
| B | **引入分裂按钮** | 3.1 分裂按钮；新增通用控件，用于"一个主动作 + 多个次要入口" |
| C | **审批卡 / 中断表单 / 过程展开态保持现有 UI** | 3.2 三节标记 🔒；**结构不重画**，只对齐颜色/字体/字号/圆角/间距 |
| D | 浅色采用"白底 + 描边"的面语言 | 规则 2 + `--surface-*` 令牌 + 3.4/3.5/3.6 |
| E | 发送键/停止键用反色单色（非强调色） | 3.2 composer |

### 6.2 仍需补参考（按价值排序）

补上后把对应章节的 🟠/🟡 升级为 🟢，并把推导值替换成锚定值。

| # | 需要什么 | 影响章节 | 为什么需要 |
|---|---|---|---|
| 1 | **含「有序列表 / 引用块 / 任务列表」的正文截图**（无序列表与表格已锚定） | 4.5 | 这三种节点仍是纯推导 |
| 2 | **界面级表格控件**（交付物列表 / 用量统计 / 审计日志任一） | 3.5 表格 / 3.6 图表 | 注意：**正文里的 markdown 表格已锚定**（4.5），缺的是界面级表格（排序 / 分页 / 选中 / 空态）与图表 |
| 3 | **空态与检索无结果**（列表空、搜索无结果） | 3.7 | 决定空态文案层级与插画取舍 |
| 4 | **拖拽态**：文件拖入输入区/窗口、页签重排、队列项拖动 | 3.2 / 3.1 | 拖拽反馈（描边？高亮？幽灵卡片？）无约定 |
| 5 | **窄窗口形态**（左栏折叠、面板并入、标题栏变化） | 3.1 | 现有截图都是 1400px 以上的宽窗口 |
| 6 | **图标组**（侧栏 + 工具条各一组，最好矢量导出） | 1.2 图标 | 核对线宽/圆角风格是否与仓内 90 个自绘 SVG 一致 |

**待确认：已全部关闭**（2026-10-15）。

### 6.3 首次视觉巡检发现的问题（2026-10-15）

按 §8 的清单跑了 `pnpm run ui:shots`（9 张）逐张看过，**令牌层落地效果符合规范**（浅色白底+描边、
深色 `#181818`、发送键反色单色、开关/主按钮/语义色统一、字号 15/14）。以下是**尚未裁决或尚未落地**的差异，
落地时按"先问再改"处理：

| # | 现象 | 与规范的关系 | 处置建议 |
|---|---|---|---|
| 1 | 正文表格**表头有底色** | §4.5 写的是"表头无底色 + 更明显的下边框" | 待裁决：实现（库主题默认）比规范更好看一档；要么改规范，要么改样式 |
| 2 | 正文代码块**有行号** | §4.3 写的是"无行号（行号只在文件预览）" | 待裁决：行号对"引用某行"很有用，参考实现没有 |
| 3 | ~~代码块头部有 4 个控件~~（折叠/复制/下载/全屏） | §4.3 最新要求只保留复制 | **已落地**：关闭折叠、下载、全屏；工具条与代码同底且无分隔线 |
| 4 | 权限 chip 是**绿色盾牌**（手动授权） | §3.2 写的是"文字与图标同色、只有高危态用 warning" | 建议改规范：绿色表达"安全/受限"是恰当的语义用法，比中性灰更好懂 |
| 5 | ~~设置页左栏宽度为 0~~ | §3.6 要求左栏 220–240px | **已修（2026-10-15）**：根因是"打开页签自动收起侧栏"（`stores/tabs.ts` 的 `maybeCollapseSidebar`）——而设置左栏就是侧栏内容，于是被压成 0。修法：`layouts/AppLayout.vue` 在设置路由上强制展开（只覆盖显示、不写回 store）。实测 `siderW: 260` 且左栏文本完整 |
| 6 | ~~设置页无卡片分组、内容列超过 720px~~ | §3.6 要求卡片 + 最大宽 720px | **已落地（2026-10-15）**：分区成为卡片（`--surface-bg` + 描边 + 14px 圆角，最大宽 720px），行分隔内缩。**取舍**：标题位于卡片外左上方，使用共享 `SettingsSection`，分组始终展开（2026-10-20 按新参考图更新） |
| 7 | ~~文件预览没有面包屑条~~ | §3.5 要求凹陷面面包屑胶囊 | **已落地（2026-10-15）**：`FilePreviewPanel` 顶部一条凹陷面胶囊，末 4 段路径 + 当前文件名加重；待改进：面包屑是按绝对路径截断的，理想是按会话工作区起算 |

**本轮裁决与确认记录（2026-10-15）**：

| 事项 | 结论 | 影响 |
|---|---|---|
| 代码块头部操作（2026-10-16） | **只保留复制**；移除折叠、下载、全屏，工具条与代码同底无分隔线 | 4.3 |
| 终端配色 | **背景与前景跟随应用主题**；ANSI 16 色另走 `theme/terminal.ts` 预设 | 3.5 |
| 链接 hover | **不加下划线**，只做颜色微调 | 4.4 |
| 弹窗主按钮 | **用 `--accent-text` 实底白字**，不跟参考图的反色单色；反色单色只留给发送键 / 停止键 | 3.3 / 3.4 |
| 破坏性主按钮 | 用 `--danger` 实底（参考图不染红，本项目取更保守的做法） | 3.3 |

（参考实现那种"反色单色主按钮"大概率源于它的 Accent 预设被设成了单色 —— 该预设的点在深色下是白点、浅色下是黑点。）

**已由已给截图解决、不再需要的**：审批卡与中断表单（改为冻结现有形态）、过程展开态（同）、浅色整页、
模式缩略图选择器、浅色菜单/下拉形态、终端外观、文件预览的语法配色与面包屑条、内置浏览器 chrome、
大标题对话框、分段控件、选项组（多选列表）、**无序列表与段落排版、文件引用 chip 的目标形态、浏览器 `⋯` 菜单**。

---

## 7. 迁移与债务

以下差异是**规范已裁决、代码尚未跟上**的部分。按"触及时顺手改"的原则收敛；成规模的改动单独立项。

**P0 —— 影响全局观感，改一处动全应用（都在 2 个文件里）**

| # | 现状 | 位置 | 规范要求 |
|---|---|---|---|
| 1 | 浅色下卡片/输入框是**灰填充无描边**（把深色语言用在了浅色） | `cards.css:33-36`、`inputs.css:23-25` | 改用 `--surface-*`：浅色白底 + 描边（规则 2） |
| 2 | 深色画布 `#141414`、右侧面板纯黑 `#000000` | `color-scheme.css:6,26` | `#181818`（1.2） |
| 3 | 深色未重定义语义色（沿用浅色值，在 `#181818` 上偏暗） | `color-scheme.css` 缺 `--success/--warning/--danger` | 补深色语义色（1.2） |
| 4 | 开关选中态两套：设置页 macOS 绿 / 其余灰 | `settings.css:105-107` / `global.css:311-317` | 统一 `--accent-text`（3.4） |
| 5 | 主按钮被全局压成灰底 | `global.css:252-262` | 主要 = 强调色实底白字；灰色改用次要按钮（3.4） |
| 6 | 字体由 Arco 注入 `Inter, …`，且 Arco 基准 14px 与本规范刻度不一致 | `arco.css:1233` / 应用无 font-family | 应用自定义 `--font-sans`/`--font-mono`（1.1） |
| 7 | 浅色设置行标题/说明是 14px/13px 量级 | 各 `settings/pages/*` | 15px / 14px（参考实现更大一档，见 1.1 注） |

**P1 —— 单点改动**

| # | 现状 | 位置 | 规范要求 |
|---|---|---|---|
| 8 | 侧栏是实底，无毛玻璃 | `frontend/src/main/index.ts:129-153` + `AppSidebar.vue` | 3.1 侧栏玻璃（决策 A） |
| 9 | 窗口 `backgroundColor: '#f7f8fa'` 写死（深色下启动闪白） | `frontend/src/main/index.ts:145` | 按主题取色；开毛玻璃后需让位 |
| 10 | 发送键颜色 | `ChatInput.vue` | 反色单色（决策 E） |
| 11 | macOS/Arco 两套语义色硬编码混用（`#34c759` 16 处、`#ff3b30` 5 处、`#0a84ff` 3 处…） | 散布 20+ 文件 | 全部走 `--success/--warning/--danger` |
| 12 | 圆角散落 7/9/10/14px | `mac-modal.css:89`、`settings.css:27`、`inputs.css:25` | 收敛到 1.3 的刻度 |
| 13 | 等宽字体 5 套并存 | `FilePreviewPanel.vue:627,685,719`、`ToolCallList.vue:275,347`、`TerminalPanel.vue:85`、`CodeEditor.vue:113` | 统一 `--font-mono` |
| 14 | 阴影多为手写 `rgba(0,0,0,x)`，`--shadow-*` 少用 | 散布 | 用令牌（1.3） |
| 15 | 空态用 Arco `a-empty` 默认插画（4 处） | 各视图页 | 3.7 的极简空态 |
| 16 | `--font-size-base: 16px` 定义了但无人使用且与 Arco 14px 冲突 | `base.css:75` | 删除或对齐刻度 |
| 17 | UnoCSS 无 theme/shortcuts（只当布局工具类用） | `uno.config.ts` | 维持现状；**不要**为了样式去加 Uno 颜色类，颜色统一走 CSS 变量 |

**P1 续：正文渲染（第 4 章，改动前先核对现状）**

| # | 现状（待核对） | 位置 | 规范要求 |
|---|---|---|---|
| 18 | 正文代码块用的可能是库的完整语法主题 | `MarkdownBody.vue` + `vue-stream-markdown` 主题 CSS | 三色极简配色（4.3） |
| 19 | 代码块头部的语言 label 形态、是否有底色/分隔线、是否有行号 | 同上 | 纯文字语言名、无分隔线、**无行号**（4.3） |
| 20 | 段落间距/行高是否为 12px / 1.7 | 同上 | 4.2 的刻度 |
| 21 | 文件预览是否已有面包屑条 | `panels/FilePreviewPanel.vue` | 凹陷面胶囊 + 当前段加粗（3.5） |
| 22 | 终端若固定了深色底或另造了背景令牌 | `panels/TerminalPanel.vue` | 背景/前景跟随主题令牌（`--bg-base` / `--text-primary`）；ANSI 16 色仍走 `theme/terminal.ts` 预设（3.5） |
| 23 | 复选框尺寸与圆角（现为 16px / 4px 量级） | `global.css:275-308` 覆盖的 Arco | 18–20px / `--radius-sm`（3.4） |
| 24 | 浏览器工具条是否有底色/分隔线、地址栏是否为常驻输入框 | `panels/BrowserPanel.vue` | 同底无下边框；导航/操作各一颗胶囊；地址栏是常驻胶囊、空态提示居中（3.5） |
| 25 | 缺少「大标题对话框」这一类骨架（现有 `mac-modal` 是 13px 小标题形态） | `components/common/mac-modal.css` | 新增任务型对话框变体（3.3） |
| 26 | 文件引用是**带底色的 chip**，无 hover 完整路径 | `components/markdown/FileRefChip.vue` | 目标形态：无底色蓝字 + 括号行号 + hover 显示完整绝对路径 tooltip（4.4） |
| 27 | 正文表格若套了容器 / 给了表头底色 / 数字列右对齐或换等宽 | `MarkdownBody.vue` + 库主题 CSS | 裸表格、表头无底色、全列左对齐、数字不换等宽（4.5） |
| 28 | 外链是否原样铺长 URL、有没有 hover 预览卡 | `components/markdown/LinkRenderer.vue` | 地球图标 + 域名；hover 弹链接预览卡（3.3 / 4.4） |
| 29 | `AppSidebar.vue:795` 项目文件夹图标仍是 `i-lucide-folder` / `i-lucide-folder-open`（线宽与端点跟手写字形不一致） | `layouts/AppSidebar.vue:795` | 换成手写 `folder` 字形（"打开态"需补字形，或用展开旋转表达）（1.6） |

**建议实施批次**（每批单独截图对照）：

1. **批次 1｜令牌层**（P0 的 1–4 + P1 的 11–14）：只动 `base.css` / `color-scheme.css` / `cards.css` /
   `inputs.css` / `settings.css` / `global.css`，全应用生效，改动面最大但风险最低。
2. **批次 2｜外壳**：侧栏毛玻璃（主进程 + 侧栏 CSS）+ 窗口底色 + 设置左栏玻璃与填充档。
3. **批次 3｜对话区**：composer 面语言与发送键、用户气泡底色、收纳头分隔线、工具执行行、变更统计徽标。
4. **批次 4｜正文渲染（第 4 章）**：代码块容器/头部/行号/三色配色 + 排版刻度 + 行内元素；
   这一批**只动正文**，是最容易看出差别、也最容易回归的一批，建议单独截图对照。
5. **批次 5｜新控件**：分裂按钮 + 选项缩略图选择器（含用到它们的第一个页面）。
6. **批次 6｜三处冻结件的令牌对齐**：审批卡 / 中断表单 / 过程展开态（只改颜色字体尺寸，不动结构）。
7. **批次 7｜终端、面包屑与浏览器 chrome**：终端独立深色令牌 + 文件预览面包屑条 + 浏览器工具条
   （与内容同底无分隔线、地址栏常态纯文本、动作按钮用强调色胶囊）。已锚定，可随时做。
8. **批次 8｜新控件补全**：大标题对话框变体、分段控件、选项组（多选列表）——三者已锚定，
   在第一个用到它们的页面里一起落地（如"清理浏览数据"或"导入数据"）。
9. **批次 9｜待参考项**：表格、空态、图表、拖拽态、窄窗口（等参考图）。

> 上面这些是**视觉规范**相关的迁移。**代码重复**（本该复用却被重写：格式化、错误提示、剪贴板、文件树状态、
> 悬浮层定位、设置页 CSS…）的完整清单与分批修整方案在 [`docs/ui-dedup-plan.md`](./ui-dedup-plan.md)，
> 配套 `pnpm run check:reuse` 基线棘轮防止长回去。

---

## 8. 改完 UI 自查清单

新增/调整任何界面后逐条过：

1. [ ] 颜色/圆角/阴影/字号全部来自令牌，**没有新增硬编码色值**（`grep -n '#[0-9a-fA-F]\{3,6\}'` 只应命中令牌文件与数据性色值）。
2. [ ] **浅色与深色都看过**：浅色下面是"白底 + 描边"，深色下面是"亮填充"，没有把一套语言用到另一模式。
3. [ ] 输入/选择类控件有描边一致性，动作类按钮是纯填充（规则 3）。
4. [ ] 强调色只出现在规则 1 的 6 类元素上（发送键是反色单色，不算强调色元素）。
5. [ ] 层级只用第 2 章的 5 档（+ 玻璃层），没有新增第 6 种底色；浮层里再套卡片时用了 `--fill-2`。
6. [ ] hover/选中/聚焦/禁用四态齐全，且**禁用态不是靠整体 opacity**。
7. [ ] 键盘 `:focus-visible` 可见；输入类有 3px 柔光环。
8. [ ] `prefers-reduced-motion` 下无位移动画；`prefers-reduced-transparency` 下无毛玻璃。
9. [ ] 文字对比度：正文 ≥ 4.5:1（`--text-secondary` 是最低档，别用它写正文）。
10. [ ] 组件类名若属于"卡片族"，已加入 `styles/cards.css` 的选择器表而不是各写一份。
11. [ ] 弹层挂在 body 上的（Arco teleport），样式写在全局文件里、且带 `app-region-no-drag`。
12. [ ] 玻璃层上的元素用 `--fill-*` 而不是纯色（否则会看不出层次）。
13. [ ] 整体观感变化 ≤ 一屏时，出**改动前后截图对照**（本仓 e2e 只守行为，不守视觉）。
14. [ ] **图标来自受控目录**：只用 `assets/ui-icons/` 的手写字形（对照 §1.6 的分类与「常见选错」）；
    没有新引入 `i-lucide-*` 类、lucide 组件或自绘内联 SVG。新增字形已按四步登记（分类会做加载期自检）。
15. [ ] **没有新增重复**：跑 `pnpm run check:reuse` 通过（可机械识别的重复量**只降不升**）；
    新写的样式/逻辑在 §1.7 查过共享件，共享层没有的先建共享件。
16. [ ] **观感变化出图**：起一份隔离实例真跑一轮对话，逐一截图欢迎页/正文/文件面板/设置页/图标页（亮深两套）并**逐张看过**。
    > 原 `pnpm run ui:shots` 巡检脚本随 2026-10-15 的整体回滚移除，需要时按本节描述重建（真跑一轮对话后逐一截图：欢迎页 / 对话正文 / 文件面板 /
    设置页 / 图标总览页 × 亮深两套，输出到 `/tmp/astorm-ui-shots`），**逐张看过**再交付 ——
    仓内 e2e 只守行为不守视觉，这张图集是唯一能替代"手点一遍"的东西。
    > 注意：脚本会等流式回复**说完**才拍（等 `.fr-chip` 会拍到表格只渲染出表头、大片空白的中间态，别改回去）。

---

## 附录 A：参考基准与取值来源

参考实现：**Codex macOS 客户端**（与 ChatGPT 桌面端同源外观）。共 **20 张**截图：

- **深色 13 张**：会话+文件面板+右键菜单、会话列表、设置「外观」、设置「通用」、命令面板、
  Markdown 正文（含代码块）、会话正文放大（无序列表 + 文件引用 tooltip）、**正文表格**、
  **外链 hover 预览卡（含消息时间戳浮标）**、终端会话、导入浏览器数据对话框、清理浏览数据对话框、浏览器 `⋯` 菜单。
- **浅色 7 张**：会话进行中、设置「通用」、设置「外观」、会话+右键菜单+子菜单、
  Markdown 正文（与深色同一内容，可作对照）、三栏工作区（对话 + 终端）、三栏工作区（对话 + 代码 + 文件树）。

它**自报**的令牌（外观设置页）——可信度最高：

| 项 | 深色 | 浅色 |
|---|---|---|
| Background | `#181818` | `#FFFFFF` |
| Foreground | `#FFFFFF` | `#1A1C1F` |
| Theme | `ChatGPT` | 同 |
| Accent | `Default`（深色下是白点、浅色下是黑点 —— 该预设本身是**单色**的） | 同 |
| Font | `System` | 同 |

按截图目测取到的其余值（已折进第 1–4 章的对应小节，此处仅留溯源）：

| 项 | 参考值 | 本文取值 |
|---|---|---|
| 画布 | 深 `#181818` / 浅 `#FFFFFF` | 采纳 |
| 前景 | 深 `#FFFFFF` / 浅 `#1A1C1F` | 深 `#f2f3f5`（刻意偏离）、浅 `#1a1c1f`（采纳） |
| 强调蓝 | ≈ `#2F6BF0`（按钮）/ `#4EA1FF`（链接） | `--accent-text`（既有 `#306fff`，已足够接近） |
| 强调色的真实用法 | 只出现在按钮/选中环/开关/链接；**发送键是黑底白箭头** | 采纳（规则 1 + 例外） |
| 面的语言 | 深色＝亮填充无描边；浅色＝白底 + 1px 浅描边 | 采纳（规则 2 + `--surface-*`） |
| 控件外形 | 输入/选择 = 面 + 描边；按钮 = 纯灰填充 | 采纳（规则 3） |
| 填充阶梯 | 白 3.5% / 7% / 10% | `--fill-1/2/3` = 5% / 9% / 15%（既有，保留） |
| 发丝线 | 深 `rgba(255,255,255,.08)` / 浅 `rgba(0,0,0,~.08)` | `--hairline` / `--surface-border` |
| 材质 | `blur(20px) saturate(180%)` | 采纳为 `--material-blur` |
| 圆角 | 控件 8 / 菜单 12 / 卡片 14 / 气泡 16 / composer 18 / 胶囊 999 | 采纳为 1.3 的刻度 |
| 字号 | 设置行标题 15 / 说明 14 / 正文 15 / 次级 13 / 导航 14 | 采纳为 1.1 的刻度 |
| 正文行高 | ≈1.65 | 采纳 |
| 正文宽度 | 无上限 | 保留 `--chat-column-max: 960px`（刻意偏离） |
| 侧栏 | 透桌面毛玻璃 | 采纳（决策 A） |
| 会话行状态 | 运行中在行尾显示小环形 spinner | 采纳（3.1） |
| 过程收纳头 | 下方一条分隔线；进行中无 chevron | 采纳（3.2） |
| 变更统计 | `7 files changed +132 -40` 胶囊 | 采纳（3.2） |
| **面的两类** | 抬升面（浅色＝白底+描边）/ 凹陷面（浅色也＝灰填充）：代码块、面包屑都是凹陷面 | 采纳（第 2 章 L1a/L1b） |
| **Markdown 代码块** | 凹陷面 + 头部与正文同底无分隔线 + 语言名为纯文字 + **无行号** + 三色配色（紫关键字/绿字符串/灰斜体注释） | 采纳（4.3） |
| **文件预览** | 有行号 + 顶部面包屑胶囊 + 完整语法配色（红橙关键字/橙棕导入名/蓝青类型/紫 class） | 采纳（3.5） |
| **终端** | 无 chrome、块状光标、文字贴边；背景与前景**跟随应用主题**（2026-10-15 用户确认） | 采纳（3.5） |
| **菜单两族** | 应用动作菜单**带左侧图标**；浏览器菜单**纯文字、更宽、行更高** | 采纳（3.3 / 3.5） |
| **菜单内嵌控件** | `Zoom` 行 = 描边胶囊里的 `− 50% +`，右侧另一个独立重置钮 | 采纳（3.5） |
| **文件引用** | 蓝图标 + 蓝字 + 括号行号、**无底色**；hover 弹完整绝对路径 tooltip | 采纳（4.4，迁移见第 7 章） |
| **无序列表** | 圆点 + **悬挂缩进** + 项间距大于行距 | 采纳（4.5） |
| **正文表格** | 裸表格、表头**无底色** + 更明显的下边框、行分隔贯穿、**全列左对齐**、数字不换等宽 | 采纳（4.5） |
| **外链** | 地球图标 + 域名、无底色无下划线；hover 弹预览卡（头部 `域名 + Open ↔` / 标题 / 摘要两行省略） | 采纳（4.4 / 3.3） |
| **消息悬浮** | 悬停消息块时弹时间戳小胶囊（`2:45` / `3:08`） | 采纳（3.2） |
| 权限 chip | 宽时"图标 + 文字"，窄时收成纯图标 | 采纳（3.2） |
| **内置浏览器** | 工具条同底无下边框；导航/操作按钮各装一颗带描边胶囊（`--bg-2`），地址栏是更亮一档的常驻胶囊（`--bg-3`）、空态提示居中；视口/缩放全部收进 `⋯` 菜单 | 采纳（3.5，2026-10-20 重锚定） |
| **大标题对话框** | 22px 标题 + 15px 说明 + 字段行 + 选项组 + 36px 高按钮；主按钮反色单色 | 结构采纳（3.3）；**主按钮颜色改为强调色**（本项目裁决） |
| **代码块头部** | 折叠指示 + 语言名 +（右侧）自动换行开关 + 复制 | 采纳（4.3，2026-10-15 用户确认） |
| **分段控件** | 不套槽的胶囊排，选中 `--fill-3`、未选透明 | 采纳（3.4） |
| **选项组** | 1px 描边容器 + 20px 图标 + 双行文本 + 右对齐复选框/开关 | 采纳（3.4） |

参考实现**没有**对应物的场景（即第 6.2 节要补的部分）：**界面级表格控件**、图表、空态、拖拽态、
窄窗口、图标组，以及 **Markdown 的有序列表 / 引用块 / 任务列表三种节点**。
