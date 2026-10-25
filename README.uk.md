[English](README.md) · **Українська**

<div align="center">

# nollm-wiki

**Карта вашого проєкту для агентів, що пишуть код: де лежить кожна функція, хто її викликає і які рядки читати. Без жодної LLM під час компіляції.**

[![CI](https://github.com/bambooplanedev/llm-wiki-nollm/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/bambooplanedev/llm-wiki-nollm/actions/workflows/rust.yml)
[![Release](https://img.shields.io/github/v/release/bambooplanedev/llm-wiki-nollm)](https://github.com/bambooplanedev/llm-wiki-nollm/releases)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

</div>

`wiki` компілює проєкт — код, нотатки, документацію — у вікі з перехресними посиланнями в Markdown
і карту символів з діапазонами рядків, а потім віддає її агентам через MCP. Агент питає `search`,
де щось лежить, і отримує рядки `path:start-end`, питає `callers`, хто викликає функцію, і читає
здебільшого саме ці рядки, а не шукає через grep і не відкриває файли цілком. Під час компіляції жодна модель
не викликається: той самий вхід дає побайтово той самий результат на будь-якій машині.

Це не семантичний пошук: зіставлення лексичне, за іменами, словами та імпортами. І не мовний сервер:
виклики розвʼязуються за іменем і кількістю аргументів, без типів.

```
$ wiki callers calls.rs resolve_calls --dir .wiki
callers of resolve_calls  wiki/src/calls.rs:195-262 — 1 functions in 1 files
wiki/src/lib.rs
  137-437  fn  compile_inner  calls resolve_calls at 302
```

## Зміст

- [Можливості](#можливості)
- [Вимоги](#вимоги)
- [Встановлення](#встановлення)
- [Швидкий старт](#швидкий-старт)
- [Використання](#використання) — [агенти](#агенти): [`wiki setup`](#wiki-setup) ·
  [Cursor](#cursor) · [Codex](#codex) · [інші](#інші-mcp-клієнти) — [MCP-сервер](#mcp-сервер) ·
  [памʼять агента](#памʼять-агента) · [CLI](#cli)
- [Що створюється](#що-створюється) — [сторінка](#сторінка) · [файли](#файли) ·
  [що читають агенти](#що-читають-агенти) · [Obsidian і Quartz](#obsidian-і-quartz) ·
  [ідентифікатори сторінок](#ідентифікатори-сторінок)
- [Виміряне](#виміряне)
- [Обмеження](#обмеження)
- [Розробка](#розробка)
- [Ліцензія](#ліцензія)

## Можливості

- **Пошук символів.** `search` ранжує сторінки за іменами, словами, документацією та імпортами,
  а під кожною сторінкою коду показує функції й типи, що збіглися, у вигляді
  `path:start-end  name  signature — doc`.
- **Структура файлів.** `page` друкує по рядку на кожен символ файлу з його діапазоном рядків, тож агент
  читає сорок рядків замість чотирьох тисяч.
- **Виклики в обидва боки.** `callers` показує функції, які викликають функцію, з рядками кожного
  виклику; `neighbors --symbol` — усе, що функція викликає, рівень за рівнем.
- **MCP-сервер.** `wiki serve --source .` компілює проєкт, перекомпільовує його після правок і віддає
  `search`, `page`, `neighbors` і `callers` у Claude Code, pi, Cursor, Codex чи будь-який MCP-клієнт.
- **Налаштування однією командою.** `wiki setup` встановлює сервер і те, що каже агентові ним
  користуватися, для Claude Code, pi, Cursor і Codex, а з `--hook` додає до кожного запиту відповідні
  рядки коду.
- **Памʼять агента.** Нотатки, які агент пише в `.agents/memory/` (або в теку користувача), стають
  сторінками для пошуку, що ранжуються вище за код, з датами, псевдонімами та `supersedes:`.
- **Мови.** Rust, Python, C++, Go, JavaScript, TypeScript, скрипти `.vue` і `.svelte`, Markdown і
  простий текст.
- **Детермінізм.** Побайтово однаковий результат між запусками, машинами й кількістю потоків;
  інкрементальна перекомпіляція.

## Вимоги

- macOS (Apple silicon чи Intel) або Linux x86_64 для релізного бінарника. Бінарник для Linux
  зібрано на `ubuntu-latest` від GitHub, тож на старішому дистрибутиві через glibc може знадобитися
  збірка з коду.
- Або Rust 1.88 чи новіший, щоб зібрати з коду. Windows не перевірявся.
- Для агентів: MCP-клієнт — Claude Code, pi, Cursor, Codex, Claude Desktop чи інший. `wiki setup --scope local`
  для Claude Code також потребує CLI `claude` у `PATH`.
- Для локальної памʼяті агента: `git` і репозиторій хоча б з одним комітом.

## Встановлення

З [релізу](https://github.com/bambooplanedev/llm-wiki-nollm/releases): завантажте архів для своєї
платформи (`aarch64-apple-darwin`, `x86_64-apple-darwin` або `x86_64-unknown-linux-gnu`) і покладіть
бінарник `wiki` у теку, що є в `PATH`, бо агенти запускають його просто як `wiki`:

    curl -L https://github.com/bambooplanedev/llm-wiki-nollm/releases/download/v0.4.0/wiki-v0.4.0-aarch64-apple-darwin.tar.gz | tar -xz
    sudo mv wiki /usr/local/bin/

З коду:

    git clone https://github.com/bambooplanedev/llm-wiki-nollm
    cd llm-wiki-nollm
    cargo install --path wiki      # у ~/.cargo/bin

## Швидкий старт

У проєкті, в якому мають орієнтуватися агенти:

    cd your-project
    wiki setup --scope local      # компілює .wiki, встановлює MCP-сервер і скіл для цієї робочої копії

Далі запустіть у цій теці Claude Code або pi: інструменти `wiki` вже там, а сервер тримає `.wiki/`
в актуальному стані після ваших правок. `--scope repo` натомість пише закомічені файли для всієї
команди, а `wiki setup --remove` скасовує будь-який варіант. Cursor і Codex:
`wiki setup --agent cursor,codex`; інші клієнти — див. [Агенти](#агенти).

Щоб спробувати сам компілятор на згенерованому корпусі:

    wiki generate demo_raw --files 12 --seed 42
    wiki compile demo_raw demo_wiki
    wiki search gradient --dir demo_wiki
    wiki neighbors gradient_descent --max-tokens 800 --dir demo_wiki

```
$ wiki compile demo_raw demo_wiki
Compiled 12 pages (12 written) -> demo_wiki
Lint: 0 broken links, 1 orphans, 0 large

$ wiki search gradient --dir demo_wiki
gradient_descent  gradient_descent.txt  Gradient Descent — Gradient Descent is often paired with Learning Rate Schedule in production pipelines.
    Gradient Descent is often paired with Learning Rate Schedule in prod…
layer_normalization  layer_normalization.txt  Layer Normalization — Layer Normalization builds directly on the ideas behind Gradient Descent.
    Layer Normalization builds directly on the ideas behind Gradient Descent. When debugging Layer Normalization, engineers trac…
learning_rate_schedule  learning_rate_schedule.txt  Learning Rate Schedule — Learning Rate Schedule builds directly on the ideas behind Backpropagation.
    …tion Mechanism. Learning Rate Schedule is often paired with Gradient Descent in production pipelines. Numbers here are approxim…
```

На коді той самий пошук називає символи з діапазонами рядків. На цьому репозиторії:

```
$ wiki search "call edges" --dir .wiki --limit 1
calls  wiki/src/calls.rs  Calls — Call edges between code items: matched by name at compile time (`resolve_calls`), followed at query time (`Wiki::calls`).
  wiki/src/calls.rs:284-308  callers_index  pub(crate) fn callers_index<'a>(code: impl Iterator<Item = (&'a str, &'a [CodeItem])>) -> BTreeMap<String, Vec<Caller>>  — Every function's callers, keyed by its `path:start-end`: the `calls` edges turned around, each list tests last, then by…
  wiki/src/calls.rs:341-479  Wiki::call_walk  fn call_walk(&self, id: &str, symbol: &str, depth: Option<usize>, max_tokens: Option<usize>, up: bool) -> Result<String, String>  — `calls`, or with `up` `callers`.
  wiki/src/calls.rs:22-31  family  fn family(e: &Entity) -> &str  — The languages whose code calls each other: a page's code language, with every JavaScript dialect and component format a…
```

## Використання

### Агенти

Агент працює так: пошук → структура → читання рядків. `search` знаходить сторінку та її символи, `page`
показує символи файлу з діапазонами рядків, і агент читає лише ці рядки своїм інструментом для
файлів. `callers` відповідає на «що зламається, якщо це змінити»; `neighbors` — коли важливі сусідні
сторінки.

| агент | що записує `wiki setup` | що каже йому користуватися вікі |
|---|---|---|
| Claude Code | `.mcp.json` (repo) або `claude mcp add-json` (local), скіл `wiki-memory` | інструкції сервера; інструменти завантажуються одразу |
| pi | `.pi/mcp.json`, скіл `wiki-navigation` | скіл |
| Cursor | `.cursor/mcp.json` | описи інструментів (у наших прогонах їх вистачило) |
| Codex | `codex mcp add` (глобально), [правила навігації](#правила-навігації) в `AGENTS.md` | `AGENTS.md` |
| будь-який інший MCP-клієнт | — ([вручну](#інші-mcp-клієнти)) | правила навігації в його файлі інструкцій |

#### `wiki setup`

`wiki setup` налаштовує агентів, щоб вони самі користувалися вікі. Запускайте в корені проєкту:

    wiki setup [--agent claude,pi,cursor,codex] [--scope repo|local] [--hook] [--remove]

Команда компілює `.` у `.wiki`, потім встановлює MCP-сервер (`wiki serve --source .`) і те, що
каже кожному агентові ним користуватися. pi отримує скіл `wiki-navigation` (як шукати код і правила
памʼяті). Claude Code отримує лише скіл `wiki-memory` з правилами памʼяті: інструкції сервера, де ті
самі кроки навігації, Claude Code показує й так, а сервер позначає свої інструменти для
завантаження одразу, а не через `ToolSearch`. Codex не читає ні того, ні того, тож отримує правила
навігації в `AGENTS.md`, між маркерами `<!-- wiki:navigation -->`. Агенти визначаються автоматично
(`.claude/`, `CLAUDE.md` чи `claude` у `PATH`; `.pi/` чи `pi` у `PATH`; тека `.cursor/`), якщо
`--agent` їх не називає. Codex налаштовується лише тоді, коли його названо: його MCP-конфіг
глобальний.

- `--scope repo` — закомічені файли для всієї команди: `.mcp.json`, `.pi/mcp.json`,
  `.cursor/mcp.json`, `AGENTS.md`, `.claude/skills/wiki-memory/SKILL.md`,
  `.agents/skills/wiki-navigation/SKILL.md` і `.wiki/` у `.gitignore`.
- `--scope local` — лише ця робоча копія: сервер для Claude Code додається через
  `claude mcp add-json wiki … --scope local`, інші файли записуються, а ті, що setup створив,
  вносяться в `.git/info/exclude`. Файл, який git уже відстежує, пропускається; для нього беріть
  `--scope repo`.
- Обидва варіанти додають сервер для Codex командою `codex mcp add wiki -- wiki serve --source .`,
  яка пише в `~/.codex/config.toml`: MCP-конфігу на рівні проєкту Codex не має, а сервер запускає в
  теці, де запущений сам, тож один запис працює для всіх проєктів.
- Без `--scope` setup питає в терміналі, а без термінала завершується з помилкою.
- `--hook` — ще й додає рядки коду з вікі до кожного запиту, перш ніж його побачить модель. Claude
  Code отримує запис `UserPromptSubmit` (`wiki hint --claude-hook`) у `.claude/settings.json`
  (`settings.local.json` з `--scope local`); pi — `.pi/extensions/wiki-hint/index.ts`. Хук мовчить,
  коли вікі немає, її не прочитати або нічого не збіглося, і ніколи не блокує запит.
- `--remove` видаляє рівно те, що записав setup, разом із хуками навіть без `--hook`, а сервер Codex
  — лише якщо це той, що додав setup. Рядки в ignore-файлах, `.wiki/` і нотатки памʼяті лишаються.

Кожен агент запускає сервер просто як `wiki`, тож `wiki` має бути в `PATH`; setup попереджає, якщо
його там немає. Запускати двічі безпечно. Запис, файл чи блок на тому самому місці з іншим вмістом
лишається як є, і setup про нього повідомляє, робить решту й виходить з кодом 1. JSON-конфіг, який
редагує setup, переписується з відступом у 2 пробіли, ключі та їхній порядок зберігаються.

Після setup: pi завантажує файли проєкту лише після того, як ви довірите проєкт (`/trust` або
`--approve` у режимі print), а Cursor завантажує новий сервер лише після вашого схвалення (у
налаштуваннях MCP або командою `cursor-agent mcp enable wiki`).

#### Claude Code вручну

    claude mcp add-json wiki '{"command":"wiki","args":["serve","--source","."]}' --scope local

або запис `mcpServers` з розділу [MCP-сервер](#mcp-сервер) у `.mcp.json` проєкту. Claude Code
показує інструкції сервера з кроками навігації, тож для пошуку коду скіл не потрібен; правила
памʼяті — у розділі [Памʼять агента](#памʼять-агента).

#### pi вручну

`.pi/mcp.json` у проєкті:

```json
{ "mcpServers": { "wiki": { "command": "wiki", "args": ["serve", "--source", "."], "exposure": "direct" } } }
```

pi не показує інструкцій MCP-сервера для серверів `direct`, тож кроки навігації йому потрібні як
скіл: покладіть [правила навігації](#правила-навігації) в
`.agents/skills/wiki-navigation/SKILL.md`. pi завантажує і те, і те лише після `/trust`.

#### Cursor

`.cursor/mcp.json` у проєкті (або `~/.cursor/mcp.json` для всіх проєктів) приймає той самий запис:

```json
{ "mcpServers": { "wiki": { "command": "wiki", "args": ["serve", "--source", "."] } } }
```

Cursor запускає сервер у теці проєкту, тож `--source .` працює. Новий сервер вимкнений, доки ви
його не схвалите, а Cursor скидає це схвалення щоразу, коли змінюється команда в записі; тоді сервер
мовчки не завантажується. У наших прогонах агент викликав `search` і `callers` без додаткових
інструкцій.

`cursor-agent -p` (неінтерактивний режим) відхиляє кожен виклик MCP-інструмента, якщо не запущений з
`--force`; `--mode ask --force` дозволяє інструменти вікі й однаково не дозволяє редагувати файли.
Перевірено з `cursor-agent` 2026.10.22; сам редактор Cursor не перевірявся.

#### Codex

Вручну `codex mcp add wiki -- wiki serve --source .` записує таке в `~/.codex/config.toml`:

```toml
[mcp_servers.wiki]
command = "wiki"
args = ["serve", "--source", "."]
```

Інструменти вікі позначені як «лише читання», тож Codex запускає їх без запитів, зокрема в
`codex exec`. Але сам по них не тягнеться: йому потрібні [правила навігації](#правила-навігації) в
`AGENTS.md` проєкту. На трьох питаннях про цей репозиторій (codex-cli 0.164.0, пісочниця лише для
читання) без них він не викликав вікі жодного разу й один раз назвав не того викликача; з ними він
користувався `search`, `page` і `callers` і правильно відповів на всі три.

#### Інші MCP-клієнти

Додайте запис `mcpServers` з розділу [MCP-сервер](#mcp-сервер) у конфіг клієнта. Клієнтові, який
стартує не в теці проєкту, як Claude Desktop, потрібен абсолютний шлях до проєкту в `--source`.
Правила навігації покладіть у його файл інструкцій.

#### Правила навігації

Те, що `wiki setup` кладе в `AGENTS.md` для Codex, — для будь-якого агента, якому вони потрібні у
файлі інструкцій. Англійською, бо їх читає агент:

```
## Find code with the project wiki

1. Call the wiki `search` tool with the words of what you are looking for and `kind: "code"`.
   Each symbol line is `path:start-end  name  signature`.
2. Read only those lines with your file tool. The start and end lines are exact — do not
   re-derive them with grep.
3. For a module's map, call `page` with the path; it lists every symbol with its lines. To follow
   a function through the code, call `neighbors` with the path and `symbol`. To see what calls a
   function, call `callers` with the path and `symbol` instead of grepping for its name.
4. Only if search finds nothing relevant, try other words, then fall back to grep.
```

### MCP-сервер

`wiki serve` віддає скомпільовану вікі MCP-клієнтам через stdio. `wiki setup` пише цю конфігурацію
за вас; вручну вона така:

```json
{
  "mcpServers": {
    "wiki": { "command": "wiki", "args": ["serve", "--source", "."] }
  }
}
```

`--source .` працює для клієнтів, що стартують у теці проєкту, як Claude Code і pi; для решти,
наприклад Claude Desktop, вкажіть абсолютний шлях до проєкту.

    wiki serve [--source <project>] [--dir <out>] [--tools <list>] [--memory <none|DIR>]

- `--source <project>` — ще й компілювати цей проєкт і тримати вікі в актуальному стані.
- `--dir <out>` (`<project>/.wiki` з `--source`, інакше `out`) — скомпільована вікі, яку віддавати.
- `--tools <list>` (`search,page,neighbors,callers`) — які інструменти відкрити, через кому, з
  `search`, `page`, `neighbors`, `callers`, `lint`.
- `--memory <none|DIR>` — з `--source`: `none` не бере локальну теку памʼяті цього репозиторію, а
  `DIR` бере іншу замість неї; `DIR`, якої немає на момент старту, не відстежується.

| Інструмент | Параметри | Повертає |
|---|---|---|
| `search` | `query` (порожній разом із `tag` дає список сторінок з тегом), `tag?`, `kind?` (`code`, `docs`, `text`, `markdown`, `code:<lang>`), `limit?` (5 із запитом; без обмеження для списку за тегом), `format?` (`lines` за замовчуванням або `json`) | Текст `lines`, описаний у [search](#search), або `[{id, title, summary, updated, score, snippet, path, symbols}]` з `json` |
| `page` | `id` (ідентифікатор, назва чи шлях), `full?` (false), `max_tokens?` (4000 для структури) | Структура або вся сторінка |
| `neighbors` | `id`, `depth?` (1), `max_tokens?` (4000), `max_nodes?`, `full?`, `kind?`, `symbol?` | Пакет контексту, або з `symbol` — функції, які викликає ця функція |
| `callers` | `id`, `symbol`, `depth?` (1), `max_tokens?` (4000) | Функції, що викликають `symbol`, з рядками кожного виклику |
| `lint` (з `--tools`) | — | JSON `{total_pages, broken_links, orphans, large}` |

Інструменти повторюють CLI, але з меншими значеннями за замовчуванням: `max_tokens` — 4000 там, де
CLI не обмежує, а `search` повертає 5 результатів замість 10, бо кожен рядок результату агент
перечитує на кожному наступному ході. `lint` відкривається лише тоді, коли його названо в
`--tools`: він потрібен тому, хто доглядає вікі, а кожен відкритий інструмент коштує токенів схеми
в кожному запиті.

**Ресурси:** кожна сторінка — `wiki://page/<id>` (Markdown), плюс `wiki://index` (`index.json`) і
`wiki://llms.txt`.

**Актуальність.** З `--source` сервер компілює проєкт сам. Наявну вікі він віддає одразу, а
компіляція йде у фоні; якщо вікі немає, перша компіляція йде до старту, і помилка завершує сервер.
Далі він стежить за проєктом і інкрементально перекомпільовує після правок (ігноровані шляхи на
кшталт `target/`, тек збірки та `.git` цього не запускають). Кожен виклик інструмента чекає до 5 с,
поки скомпілюються правки, зроблені до нього. Якщо перекомпіляція впала або ще йде, сервер віддає
останній вдалий знімок, а до результатів додається рядок `wiki may be stale: <reason>`. Короткочасні
файли-барʼєри, по одному на виклик, лежать у `<dir>/.serve/`. Без `--source` сервер лише читає
вікі: перед кожним запитом він звіряє відбиток `index.json` і перечитує вікі після перекомпіляції;
якщо перечитати не вдалося, лишається попередній знімок, а спроба повторюється на наступному запиті.

### Памʼять агента

Агент може тримати свої нотатки як Markdown-файли в теці, яку компілює вікі, і шукати в них через
`search`, перш ніж писати нове. `wiki setup` створює теку й розповідає про неї агентові у скілі:

- **Для репозиторію** (`--scope repo`): `.agents/memory/` у проєкті. Тека комітиться, тож нотатки
  бачать усі, хто має репозиторій. `wiki compile` завжди її включає.
- **Локально** (`--scope local`): тека поза робочою копією, у теці даних користувача:
  `%LOCALAPPDATA%\wiki\memory\<key>` на Windows, `~/Library/Application Support/wiki/memory/<key>`
  на macOS, `$XDG_DATA_HOME` (інакше `~/.local/share`)`/wiki/memory/<key>` деінде. `<key>` — це
  кореневий коміт репозиторію, тож усі worktree і клони одного репозиторію ділять одну теку. Резервних
  копій цієї теки ніщо не робить, і вона переживає `git clean`. Тека, що не є git-репозиторієм, локальної памʼяті не
  має.

`serve --source` і `wiki setup` включають локальну теку. Звичайний `wiki compile . out` — ні, щоб
опублікований сайт ніколи не ніс приватних нотаток; для нього передайте `--memory local` або
`--memory DIR` для будь-якої теки (якщо теку не знайдено, буде попередження, і компіляція піде
далі). Тож звичайний `wiki compile . .wiki` прибирає локальну памʼять з `.wiki` до наступної
перекомпіляції сервером, а `wiki hint` читає саме `.wiki`.

Сторінки памʼяті ранжуються вище за код (їхня оцінка множиться на 4), проходять фільтри `code` і
`docs` у `search`, і їх знаходять ті самі інструменти, що й будь-яку сторінку. Коли запит називає
нотатку, `wiki hint` додає до рядків коду один рядок `memory:` (див. [hint](#hint)).

Дата в запиті знаходить нотатки памʼяті за той день: `12 жовтня`, `October 12`, `12.10`,
`2026-10-12` (українські, російські та англійські назви місяців; `DD.MM` потребує двох цифр дня і
місяця; без року підходить будь-який рік). Сторінки проєкту за датою не знаходяться ніколи.

Нотатка може замінити старішу через `supersedes:` у front matter (назви, псевдоніми чи
ідентифікатори, у синтаксисі `aliases:`). Тоді старіша нотатка втрачає підсилення, `search` дописує
` (superseded by <id>)` після її рядка (`"superseded_by"` у JSON), а `wiki hint` її ніколи не показує.
Зіставляються лише нотатки памʼяті, імʼя, що ні з чим не збіглося, ігнорується, а коли таких нотаток
кілька, показується найменший ідентифікатор.

Front matter будь-якої Markdown-сторінки: `aliases:` і `tags:` пишуться як `a, b`, `[a, b]` або
YAML-список; теги переводяться в нижній регістр, `#` відкидається. Дата сторінки береться з
`updated`, `modified` чи `date`, потім з тих самих ключів у блоці `metadata:` на один рівень
глибше, потім з назви файлу, що починається з `YYYY-MM-DD`. Кириличні слова запиту зводяться до
основи, тож інша граматична форма слова теж дає збіг.

Скіл містить ці правила; якщо в агента скілів немає, скопіюйте їх у його файл інструкцій. Правила
англійською, бо їх читає агент:

```
Memory notes live in <folder> (.agents/memory, or the local folder setup named), one markdown file per fact or topic.
- Keep a note under about 16,000 characters (`wiki lint` reports `large` above that).
- Name the file for what it says. The title is the front-matter `title:`,
  else the first `# heading`, else the file name; any script works.
- A note not written in English needs English `aliases:`: agents search in English.
- Front matter, all optional: `title:`, `description:` (one line, the search
  summary), `aliases:`, `tags:`, `updated: YYYY-MM-DD` (date of the last change; other formats are
  ignored).
- A journal note is named `YYYY-MM-DD.md` and has the date in its H1.
- Mention a related note by its exact title or alias, or as `[[title]]`.
- Search before writing. Update the note that exists instead of adding a copy.
- When a decision changes, update its note. If you write a new note instead, add `supersedes: <old note's title>` to it.
- `MEMORY.md` is optional and is not searched.
```

### CLI

    wiki [--jobs N] <command>

| команда | що робить |
|---|---|
| `compile` | компілює проєкт у вікі |
| `search` | сторінки й символи за запитом |
| `page` | структура файлу: по рядку на символ з діапазоном рядків |
| `callers` | функції, що викликають функцію |
| `neighbors` | пакет контексту навколо сторінки в межах бюджету, або з `--symbol` — що викликає функція |
| `lint` | биті посилання, сторінки-сироти, сторінки, завеликі для пакета |
| `serve` | MCP-сервер — див. [MCP-сервер](#mcp-сервер) |
| `hint` | рядки коду для тексту задачі, для хуків запиту |
| `setup` | встановлює сервер для Claude Code, pi, Cursor і Codex — див. [Агенти](#агенти) |
| `generate` | синтетичний корпус для демо й звітів про помилки |

`--jobs N` задає кількість потоків для `compile` (за замовчуванням як у Rayon, зазвичай за числом
ядер); результат від цього не залежить. Команди-запити читають `--dir <out>` (за замовчуванням
`out`) і ніколи не перекомпільовують.

#### compile

    wiki compile [OPTIONS] <INPUT> <OUTPUT>

- `--incremental` — брати відбитки рендерингу з `.wiki/cache.json` і пропускати незмінені сторінки.
- `--no-ignore` — не застосовувати правила `.gitignore`/`.ignore`/прихованих файлів під час обходу
  `<INPUT>`.
- `--emit-json` — ще й записати `graph.json` (граф вузлів і ребер).
- `--project <NAME>` (за замовчуванням імʼя вхідної теки) — назва проєкту в `index.json`,
  `llms.txt` і `AGENTS.md`. Без нього `compile .` і `compile "$PWD"` назвуть проєкт по-різному.
- `--memory <local|DIR>` — ще й компілювати теку памʼяті агента як позначені сторінки памʼяті:
  `local` — тека цього репозиторію в теці даних користувача, або будь-яка тека. `<INPUT>/.agents/memory`
  компілюється завжди.
- `--watch` — скомпілювати раз, а потім стежити за `<INPUT>` і перекомпільовувати після змін. Події
  групуються з затримкою 150 мс, події під `<OUTPUT>` ігноруються, а кожен прогін пише
  `recompiled: N pages (M written)` у stderr.

Якщо в `<OUTPUT>` ще лежать сторінки, записані за старою схемою ідентифікаторів (давніша версія або
перейменовані відтоді джерела), `compile` попереджає в stderr і лишає їх на місці: видаліть їх або
компілюйте в чисту теку.

#### search

    wiki search [--kind K] [--tag T] [--limit N] [--format lines|json] [--dir out] [QUERY]

- `--kind <k>` — `code` (усі `code:<lang>`), `docs` (`markdown` і `text`) або один точний тип:
  `text`, `markdown`, `code:<lang>` (наприклад, `code:rust`). Нотатки памʼяті проходять і `code`, і
  `docs`.
- `--tag <tag>` — лише сторінки з цим тегом (`#` і регістр не враховуються; `a` збігається і з `a/b`). Без
  запиту дає список усіх таких сторінок. Потрібен запит або `--tag`.
- `--limit <N>` (`10` із запитом) — максимальна кількість результатів.

За замовчуванням — по рядку на сторінку: `id  path  [date  ]title — summary`. Під сторінкою коду до
3 рядків з відступом називають символи, що збіглися: `  path:start-end  name  signature  — doc`
(нумерація з 1, межі включно; для методів `name` має вигляд `Owner::name`). Показуються лише символи, що набрали
щонайменше 30% від найкращого символу у відповіді, а тестовий код рахується з половинною вагою.
Сторінка без рядків символів натомість отримує уривок тексту в рядку з відступом у чотири пробіли.
`--format json` друкує `[{id, title, summary, updated, score, snippet, path, symbols}]`, кожен
символ — `{path, start_line, end_line, name, signature, doc}`.

#### page

    wiki page [--full] [--max-tokens N] [--dir out] <ID>

`<ID>` — ідентифікатор сторінки або будь-що, що однозначно називає одну сторінку: її назва (у
будь-якому регістрі), slug назви, шлях до джерела чи унікальний кінець шляху (`query.rs`) або
абсолютний шлях усередині скомпільованої теки. Якщо нічого не збіглося, помилка перелічує найближчі
ідентифікатори. `neighbors` і `callers` розвʼязують `<ID>` так само.

```
$ wiki page calls.rs --dir .wiki
# Calls (calls)
wiki/src/calls.rs  code:rust
Call edges between code items: matched by name at compile time (`resolve_calls`), followed at query time (`Wiki::calls`).
imports:
  crate::query::Wiki
  …
15-20  fn  fn owner_key(owner: &str) -> &str  — The last segment of an owner, generics dropped: `Outer.Inner` and `a::Inner<T>` are both `Inner`.
36-48  type  struct Index<'a>  — What call sites resolve against: every function by name, every page by module name, every type name.
Index:
  51-104  method  fn new(entities: &'a BTreeMap<String, Entity>) -> Self
  …
195-262  fn  pub fn resolve_calls(entities: &mut BTreeMap<String, Entity>, graph: &Graph)  — Fills every function item's `calls` from its `call_sites`, which it empties.
```

Рядок елемента — `start-end  kind  signature`, далі `  → twin` для парного C++-оголошення чи
визначення і `  — doc`, коли є речення документації. Методи стоять під рядком `owner:`; тестові
елементи позначені `[test]`. Сторінка Markdown показує свої `headings:`, текстова — лише шапку.

Коли `--max-tokens` менший за структуру, частини відрізаються в такому порядку, поки не вміститься:
тестові елементи, потім документація, потім імпорти, потім елементи з кінця, а останній рядок каже,
що прибрано (`… dropped: 3 test items, 5 docs — pass max_tokens: 2412 for all`). З `--full` і
бюджетом, меншим за сторінку, сторінка коду друкує структуру в межах бюджету; будь-яка інша обрізається
по цілих рядках.

#### callers

    wiki callers [--depth N] [--max-tokens N] [--dir out] <ID> <SYMBOL>

Функції, що викликають функцію `<SYMBOL>` (`name` або `Owner::name`) на сторінці `<ID>`, по рядку
`start-end  kind  Owner::name` на кожну, згруповані за файлами; рядок закінчується на
`calls f at L1, L2` з рядками, де стоїть виклик, а тестовий код іде в кінці. `--depth 2` додає
викликачів викликачів; з `--max-tokens` першими відрізаються найдальші.

#### neighbors

    wiki neighbors [--depth N] [--max-tokens N] [--max-nodes N] [--full] [--kind K] [--symbol NAME] [--dir out] <ID>

Пакет контексту для агента в межах бюджету: обхід у ширину від `<ID>` на `--depth` кроків (1).
`--max-tokens` — жорстка межа оціненого розміру пакета (знаки/4), `--max-nodes` обмежує кількість
вузлів; сусіди беруться за центральністю (PageRank), а той, що не вміщується в залишок бюджету,
пропускається, і обхід іде далі. Ціль іде першою — сторінка коду як структура, — а сусіди за нею в
порядку центральності, найважливіші останніми, щоб найцінніший контекст стояв ближче до кінця. Без
`--full` першими йдуть модулі, які ціль імпортує або які імпортують її, — структурою, якщо вміщуються,
— а решта одним рядком опису.

`--symbol <NAME>` натомість друкує всі функції, які ця функція викликає, на всіх рівнях (або на
`--depth` з них), у форматі рядків `callers`.

#### lint

    wiki lint [--dir out]

Друкує кількість битих посилань, сторінок-сиріт і завеликих сторінок (понад 4000 токенів: таку
сторінку пакет `neighbors` за замовчуванням урізав би до опису; сторінки коду не перевіряються), а потім по
рядку на кожну. `[[посилання]]` в огороджених блоках, вбудованому коді та вихідному коді сторінки коду
не рахуються. Виходить з кодом 1, коли є бите посилання, тож `lint` може зупиняти збірку.

#### hint

    wiki hint [--dir .wiki] [--claude-hook] [TEXT]

Місця в коді для тексту задачі, як їх вставляє хук запиту: рядок-заголовок, далі до 8 рядків
`path:start-end  name  signature` з найкращих сторінок коду, не більше 2 на сторінку, без
документації, тестів і рядка, діапазон якого містить інший рядок тієї ж сторінки. Нічого не друкує й
виходить з кодом 0, коли вікі немає або нічого не збіглося, тож хук може запускати його на кожен
запит. `--claude-hook` читає JSON хука `UserPromptSubmit` з Claude Code зі stdin замість `TEXT`, і
тоді `--dir` за замовчуванням — `$CLAUDE_PROJECT_DIR/.wiki`.

Коли запит називає нотатку памʼяті її назвою, псевдонімом чи описом, після рядків коду йде один рядок
`memory: <path> — <summary>`. Для нього потрібне слово запиту з шести чи більше літер у цьому
збігу, або з чотирьох-пʼяти літер, якщо нотатка набрала щонайменше вдвічі більше за найкращу
сторінку коду (до підсилення памʼяті), або дата в запиті, що збігається з днем `updated` нотатки.

#### generate

    wiki generate [--files 20] [--seed 42] <DIR>

Синтетичний корпус: той самий seed і `--files` завжди дають побайтово ті самі файли.

## Що створюється

### Сторінка

Кожен файл-джерело стає однією сторінкою. Усі розділи належать компілятору, крім `## Notes`: те, що
ви там напишете, переживає перекомпіляцію.

```
$ wiki page gradient_descent --full --dir demo_wiki
# Gradient Descent

<!-- generated by wiki compiler — do not edit compiler-owned sections -->

## Metadata
- created: 2026-01-10
- aliases: gradient_descent, gradient_descent_notes
- kind: text
- source: gradient_descent.txt
- source_hash: 66fce8283cb6b5e45c6a3ee6b6c3e34204c9ba9c562f4d3df2dfdd7a3dd33c79

## Related
- [[learning_rate_schedule|Learning Rate Schedule]]

## Mentioned in
- [[layer_normalization|Layer Normalization]]
- [[learning_rate_schedule|Learning Rate Schedule]]

## Body
Gradient Descent is often paired with Learning Rate Schedule in production pipelines.

Revisit this after the next benchmark run.

## Notes
_(add your own notes here — preserved on recompile)_
```

Текст сторінки коду — це її вихідний код; символи з діапазонами рядків лежать в `items.json` і
показуються через `wiki page`.

### Файли

| Файл | Що це | Для кого |
|---|---|---|
| `<id>.md` | одна Markdown-сторінка з перехресними посиланнями на кожен файл-джерело | люди й агенти |
| `index.json` | каталог: записи, посилання, центральність | агенти й інструменти |
| `items.json` | елементи кожної сторінки коду з діапазонами рядків і викликами | агенти й інструменти |
| `index.md` | сторінка-покажчик | люди |
| `llms.txt` | стислий покажчик з описами в один рядок | моделі |
| `AGENTS.md` | як агентові користуватися цією вікі | агенти |
| `graph.json` | граф вузлів і ребер, лише з `--emit-json` | інструменти для графів |
| `.wiki/cache.json` | кеш інкрементальної компіляції | компілятор |

### Що читають агенти

- **`index.json`** — `{ project, root, entries: [...] }`, де `root` — імʼя самої скомпільованої
  теки. Кожен запис має `id`, `title`, `kind`, `aliases`, `tags`, `updated` (лише коли сторінка має
  дату), `path`, `page`, `summary` (`null`, коли опис вивести не вдалося), `degree_in`,
  `degree_out`, `pagerank`, `token_estimate`, `neighbors_out`, `neighbors_in`, `imports_out`,
  `imports_in` (ребра імпорту серед них), `defined` (імена визначень верхнього рівня) і `methods`.
- **`items.json`** — карта коду, винесена з `index.json`, щоб каталог лишався малим. Один обʼєкт з
  ключами-ідентифікаторами сторінок; кожне значення перелічує елементи сторінки з `kind`, `name`,
  `owner`, `signature`, `start_line`/`end_line`, `doc`, `public`, `twin` для парного C++-оголошення
  чи визначення, а для функції — `calls` (`path:start-end` функцій, які вона викликає) і
  `call_lines` (для кожної — рядки виклику).
- **`llms.txt`** — сторінки з PageRank не нижче медіани під `## Docs`, решта під `## Optional`.
- **`AGENTS.md`** — з чого почати, як отримати сусідів чи карту символів модуля, і що розділ
  `## Notes` кожної сторінки зберігається між перекомпіляціями.

### Obsidian і Quartz

Перехресні посилання — це вікі-посилання в стилі Obsidian з явною ціллю, `[[slug|Display Name]]`,
а кожен `<slug>.md` — справжня сторінка, тож посилання працюють без плагінів. Відкривайте як
сховище саму теку з результатом компіляції, а не батьківську, де лежать і джерела: їхні імена файлів можуть
збігтися зі сторінками. Quartz читає ті самі посилання: [`scripts/`](scripts/README.md) публікує
скомпільовану вікі як сайт на Quartz.

### Ідентифікатори сторінок

Ідентифікатори сторінок (і назви файлів) відповідають `[a-z0-9_]+`. Вони утворюються з назви
кожного джерела: розділові знаки й пробіли згортаються в `_`, латинські літери з діакритикою — до
базової, а кирилиця транслітерується (`Морський маяк` стає `morskyi_maiak`). Сторінка `mod.rs` чи
`__init__.py` називається за своєю текою (`tests/common/mod.rs` → `common`). Дві назви, що
згортаються в один ідентифікатор — `Fetch (v2)` і `Fetch v2`, два файли з однаковою основою в різних
теках, `orderbook.h` і `orderbook.cpp`, — розрізняються сегментом теки, потім розрізнювачем з назви
файлу, потім вільним ідентифікатором; жодне джерело не губиться через збіг
([докладніше](wiki/docs/ARCHITECTURE.md#disambiguation-ladder)). Назва, у якій після згортання не
лишилося жодної літери чи цифри, стає `page_<hash>`.

## Виміряне

Спершу прочитайте це: цифри взято з прогонів агентів на одному приватному проєкті на
C++/Rust/Python/TypeScript, близько 2 700 сторінок, з Claude Haiku у Claude Code, по два прогони на
питання й конфігурацію. Вони кажуть, що зробила ця модель на цих питаннях у цьому проєкті, а не що
вікі зробить у вашому, а при двох прогонах різниця в пів ходу — це шум.

`callers` і ребра викликів за ним, 2026-10-25. Усі три конфігурації мають вікі:

- `base` — `search`, `page` і `neighbors`, без `callers`;
- `callers` — додає `callers`;
- `+ macros` — ще й бачить виклики в аргументах Rust-макросів і через псевдоніми імпортів. Це
  v0.4.0.

Питання «де» — де щось робиться (12 питань); «вплив» — що викликає функцію або що зачепить зміна
(8); останні 3 — про викликачів, що сидять в аргументах Rust-макросів (`format!`, `tokio::select!`)
або за псевдонімом імпорту. Колонка «правильних прогонів» рахує прогони; ходи й вхідні токени — середні на прогін;
виклики Grep/Glob — сума за всі прогони.

| конфігурація | питання | правильних прогонів | ходів | вхідних токенів | викликів Grep/Glob |
|---|---|---|---|---|---|
| `base` | де | 23/24 | 5.25 | 118K | 30 |
| `callers` | де | 23/24 | 5.58 | 142K | 29 |
| `+ macros` | де | 23/24 | 5.12 | 119K | 24 |
| `base` | вплив | 13/16 | 4.88 | 128K | 26 |
| `callers` | вплив | 16/16 | 3.94 | 101K | 9 |
| `+ macros` | вплив | 16/16 | 4.06 | 111K | 12 |
| `callers` | викликачі в макросах чи за псевдонімами | 6/6 | 5.83 | 140K | 8 |
| `+ macros` | викликачі в макросах чи за псевдонімами | 6/6 | 4.00 | 96K | 5 |

На тому самому проєкті Haiku прочитав діапазон рядків, який дав `search`, у 57 з 58 читань файлів.
Більше, разом із випадками, коли агенти все ж повертаються до grep:
[wiki/docs/KNOWN_GAPS.md](wiki/docs/KNOWN_GAPS.md) (англійською).

## Обмеження

- **Зіставлення лексичне.** Ні ембедингів, ні семантичного пошуку: «credentials» не знаходить `creds`.
  Текстова чи Markdown-сторінка звʼязується, коли її назва чи псевдонім трапляється на іншій
  сторінці; сторінка коду — лише через згадку її модуля у вигляді коду (шлях з `::`, оголошення
  `mod`, назва файлу), імпорт або відносне Markdown-посилання.
- **Виклики розвʼязуються за іменем і кількістю аргументів**, у межах однієї мови, без типів: метод,
  викликаний на значенні невідомого типу, веде на всі однойменні методи, які бачать файли
  викликача.
- **Три формати.** Текст, Markdown і код; без PDF, OCR і аудіо.
- **MCP-сервер лише для запитів**, через stdio; перекомпільовує після правок файлів, а не на запит.
- **`--watch` є лише в `compile`**, а граф експортується лише в JSON.

Виміряні недоліки й поведінка агентів: [wiki/docs/KNOWN_GAPS.md](wiki/docs/KNOWN_GAPS.md).
Внутрішня будова, правила детермінізму й як додати мову: [wiki/docs/ARCHITECTURE.md](wiki/docs/ARCHITECTURE.md).
Обидва документи англійською.

## Розробка

    git clone https://github.com/bambooplanedev/llm-wiki-nollm
    cd llm-wiki-nollm/wiki

    cargo test             # модульні, інтеграційні, snapshot-тести й оцінка пошуку
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

`wiki` — ще й бібліотека; CLI — тонка обгортка над тим самим API, див.
[wiki/README.md](wiki/README.md). Пуш тегу `v*` збирає бінарники для Linux і macOS і прикріплює їх
до GitHub Release.

### Самохостинг

Цей репозиторій компілює сам себе. `scripts/selfhost.sh` збирає релізний бінарник, компілює
репозиторій у `.wiki/` (у gitignore, інкрементально) і запускає на ньому `wiki lint`; на битому
посиланні скрипт завершується з помилкою. Закомічений `.mcp.json` реєструє `wiki serve --source .` як сервер `nollm-wiki`, тож
агент, запущений тут, може шукати в проєкті; у свіжому клоні запустіть `scripts/selfhost.sh` один
раз, щоб бінарник зʼявився.

## Ліцензія

[MIT](LICENSE)
