# P17_SPEC — CLI 参数解析硬化（拒绝未知短标志 + `-i` 别名）· 收口 R2

前置事实（实测于 P16 之前的 HEAD `eedfe2f`，锚点为文件:行）：

- `src/main.rs:98 fn parse_flags(args: &[String]) -> (HashMap<String, String>, Vec<String>)` 是唯一参数解析器。
- `src/main.rs:108` 短标志分支条件：`a.starts_with('-') && !a.starts_with("--") && a.len() >= 2`；
  `match a[1..]` 只认 `o`(→format) / `q` / `h` / `V` / `a` / `g`，其余落进 `_ => {}`；
- 落空后继续走 `:148 if let Some(k) = a.strip_prefix("--")`，对 `-i` 失败 → `:160 pos.push(a.clone())`。
  **后果（R2 缺陷）**：`roamway run --agent shell -i /tmp/x.png "echo x"` 的 prompt 变成 `-i /tmp/x.png echo x`，
  agent shell 执行 `sh -c "-i …"` → run status=failed exit=2，报错只有 `sh: 0: Illegal option -`，而 CLI 自己 exit 0。
  一个拼错的短标志今天会静默改掉 prompt，这是必须堵的坑。
- `--image` 的唯一入口：`:346 flag_many(flags, "image")` + `:242 validate_image_path`
  （文件不存在 → `run --image: no such file: <p>`）。
- `src/main.rs:40 fn usage_error(msg)` = stderr + `fail(2)`；`:929 fn dispatch(argv: &[String]) -> i32` 第 930 行调用 `parse_flags`。
- 既有单测 `:4929 parse_flags_short_o_and_q` 直接用二元组返回值 → **`parse_flags` 的签名与语义必须原样保留**。
- 已确认没有别处依赖「单破折号未知标志当位置参数」：`-c` 只出现在 `Command::new("/bin/sh").args(["-c", …])`（:2478），与 CLI 解析无关。

本阶段目标：让未知短标志在**解析期**报错（exit 2），并把 `-i` 做成 `--image` 的正式别名。

---

## 一、语义

1. 未知短标志（单破折号、`len >= 2`、不是裸 `-`、不在已知集 `{o,q,h,V,a,g,i}` 内）→
   exit 2，stderr 三行：
   ```
   unknown flag: -x
   pass it after '--' if it is part of the prompt
   run 'roamway help' for usage
   ```
   第一行逐字为 `unknown flag: <原始 token>`（`-xvf` 报 `-xvf`，不报 `-x`）。
2. `-i <path>` ≡ `--image <path>`（`push_flag("image", next)`，i += 2）。`-i` 位于 argv 末尾无值 →
   `usage_error("run --image: missing value")`。
3. 不变量（全部要有单测）：
   - `--` 之后的 `-x` 原样进 pos，不报错；裸 `-`（len<2）仍是位置参数；
   - 单个位置参数里含 flag 模样文本不受影响：`roamway run --agent shell "echo -n hi"` → pos = `["run", "echo -n hi"]`；
   - `-o json` / `-q` / `-a` / `-g` / `-h` / `-V` 行为逐位不变（既有测试保持绿）。

## 二、实现

1. `src/main.rs` 新增
   ```rust
   struct ParsedArgs { flags: HashMap<String, String>, pos: Vec<String>, unknown: Vec<String> }
   fn parse_args(args: &[String]) -> ParsedArgs   // 现有 parse_flags 主体搬进来
   ```
   `fn parse_flags(args) -> (flags, pos)` 改成 `let p = parse_args(args); (p.flags, p.pos)` 的薄包装
   （**不要改它的调用点，也不要改它的返回顺序**）。
2. 短标志分支：已知集合走原逻辑（`i` 新增）；`_ =>` 改为
   `unknown.push(a.clone()); i += 1; continue;`。
3. `dispatch`：在 `let (flags, pos) = …` 之后、`pos.is_empty()` 判断与任何命令分派之前插入
   ```rust
   if let Some(first) = parsed.unknown.first() {
       eprintln!("unknown flag: {}", first);
       eprintln!("pass it after '--' if it is part of the prompt");
       eprintln!("run 'roamway help' for usage");
       return 2;
   }
   ```
   （`-h`/`-V` 已解析成 flag，不受影响；`roamway -x` 这种无子命令的调用也必须 exit 2，而不是打印 usage。）
4. 帮助文本：`usage()` 的 run 行与 `print_command_help("run")` 补 `[--image PATH]` 与 `[-i PATH]`；
   通用参数行末补一句：`未知短标志 exit 2；prompt 里以 - 开头的词请放在 -- 之后`。

## 三、验收（NAS 上，二进制 `$HOME/rw-src/target/release/roamway`）

| 命令 | 期望 |
|---|---|
| `roamway run --agent shell -i /tmp/nope.png "echo x"` | exit 2，`run --image: no such file: /tmp/nope.png`（别名生效且先校验） |
| 造 `/tmp/p17.png` 后 `roamway run --agent shell -i /tmp/p17.png "echo x"` | exit 0，打印 run id（shell agent，**无 token 消耗**） |
| `roamway run --agent shell -x "echo x"` | exit 2，首行 `unknown flag: -x` |
| `roamway run --agent shell -- -x y` | exit 0，`-x y` 进 prompt |
| `roamway -q ls` / `roamway -o json ls` | 与 P16 前一致 |
| `cargo test` | 全绿；新增 ≥4 条（未知短标志识别、`-i` 别名、`--` 透传、dispatch 返回 2） |

约束：不改 P1-P3 已验收行为；不动 HTTP 侧与 `run_plan` 的既有 flag 语义；只改 `src/main.rs` 与 README 帮助文本。
