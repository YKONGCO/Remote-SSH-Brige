# 全局主题：CC Switch

本应用的颜色体系采用 [CC Switch](https://github.com/farion1231/cc-switch) 当前 v7 主题，固定参考提交 `9941a6455ef9405cf9d3620057fddd283223a607`。

`src/theme-tokens.css` 采用上游 `src/index.css` 的亮暗两套主题变量，按用户要求将主操作色替换为深蓝色；其后的别名将现有组件变量映射到上游语义。`src/fluent.css` 保留组件样式与布局，名称为历史兼容；旧 WinUI 色值已移除。`src/styles.css` 的硬编码配色也已清理，中性色边框统一引用主题。

| 用途                      | 浅色      | 深色      |
| ------------------------- | --------- | --------- |
| 页面 `bg-app`             | `#ffffff` | `#1c1c1e` |
| 侧栏 `bg-sidebar`         | `#f5f5f7` | `#161618` |
| 卡片 `bg-card`            | `#ffffff` | `#2e2e33` |
| 悬停 `bg-subtle`          | `#efeff2` | `#26262a` |
| 选中 `bg-selected`        | `#e4e4ea` | `#313136` |
| 主文字 `text-1`           | `#18181b` | `#e4e4e7` |
| 辅助文字 `text-2`         | `#52525b` | `#ababb3` |
| 弱化文字 `text-3`         | `#65656e` | `#9a9aa2` |
| 主操作 `action-bg`        | `#3b70b6` | `#4278b6` |
| 主操作悬停 `action-hover` | `#3264a6` | `#386ba7` |
| 控件边框 `border-strong`  | `#cfcfd6` | `#4a4a52` |

组件状态也遵循参考实现：侧栏选中使用中性选中底；主按钮、勾选和开关使用深蓝色；次要按钮采用卡片底与中性边框；图标按钮及菜单使用 subtle 悬停底。服务器卡片悬停保留卡片底，仅增强边框与轻阴影。文件行内按钮悬停用 selected 与所在行区分。标签页选中采用参考 Tabs 的主操作底和白字，完整内嵌于圆角容器。

成功、警告、错误和服务器分组颜色分别复用上游语义变量。终端默认独立深色，画布采用深色侧栏底，栏区采用 subtle；文字及 ANSI 颜色映射到相同主题的文字和状态色。切换终端配色不重建会话。

保留用户已经指定的行为：所有输入框与下拉选项无聚焦 ring、弹窗无外围阴影、暗色文字减重、终端 4–8px 外边距、文件工具栏单行、文件时间单行、条目高度 40px，以及合并后的窗口标题区。

参考源码：[主题变量](https://github.com/farion1231/cc-switch/blob/9941a6455ef9405cf9d3620057fddd283223a607/src/index.css)、[按钮](https://github.com/farion1231/cc-switch/blob/9941a6455ef9405cf9d3620057fddd283223a607/src/components/ui/button.tsx)、[侧栏](https://github.com/farion1231/cc-switch/blob/9941a6455ef9405cf9d3620057fddd283223a607/src/components/shell/Sidebar.tsx)、[卡片](https://github.com/farion1231/cc-switch/blob/9941a6455ef9405cf9d3620057fddd283223a607/src/components/providers/ProviderCard.tsx)。MIT 许可及版权声明保存在 `docs/licenses/CC_SWITCH_LICENSE.txt`。
