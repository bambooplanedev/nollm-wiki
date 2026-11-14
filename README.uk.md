[English](README.md) · **Українська**

# kulman

**kulman** читається «кульман»: кульман — креслярська дошка конструкторського бюро; тут це вхідні
двері набору: він встановлює інструменти й налаштовує проєкт.

[![CI](https://github.com/bambooplanedev/kulman/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/bambooplanedev/kulman/actions/workflows/rust.yml)
[![Release](https://img.shields.io/github/v/release/bambooplanedev/kulman)](https://github.com/bambooplanedev/kulman/releases)
[![Rust 1.90+](https://img.shields.io/badge/rust-1.90%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

kulman — набір інструментів для агентів, що пишуть код, і людей, які з ними працюють; назви взято
з креслярських дошок і плазів авіаційного конструкторського бюро. `kalka` компілює проєкт у карту,
в якій агент може шукати, — де лежить кожна функція, хто її викликає, які рядки читати, — а `plaz`
малює справжні рішення кожної функції, щоб їх читали люди. Жоден не викликає модель: ті самі
джерела дають побайтово той самий результат, а `kulman` встановлює обидва й налаштовує їх у проєкті
однією командою на кожен крок.

## Назви

| Назва | Як читати | Що означає слово |
|---|---|---|
| kulman | «кульман» | креслярська дошка конструкторського бюро — вхідні двері: встановлює інструменти, налаштовує проєкт |
| kalka | «калька» | прозорий папір для копіювання — точна копія вашого проєкту, яку читають агенти |
| plaz | «плаз» | підлога, на якій бюро викреслює справжні лінії планера в натуральну величину — справжні рішення кожної функції |
| techlog | «теклог» | технічний журнал літака — памʼять агента про проєкт |
| wiki-code | — | проста назва: спільна бібліотека, що обходить, розбирає й індексує код |

## Встановлення

Поточний реліз — 0.7.0. Сам `kulman` завантажує один рядок:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh

Скрипт завантажує архів `kulman` для вашої системи, перевіряє його за `SHA256SUMS` релізу (і
зупиняється, якщо немає ні `sha256sum`, ні `shasum`) та кладе бінарник у `~/.kulman-home/bin/kulman`.
Більше він нічого не записує і не змінює профілів оболонки. Додайте `~/.kulman-home/bin` у свій
`PATH` самі: агенти запускають `kalka` за назвою.

Далі встановіть модулі:

    kulman install

Ця команда кладе `kalka` і `plaz` (модулі релізу за замовчуванням) у `~/.kulman-home/bin`, поруч із
`kulman`, а в `~/.kulman-home` — `manifest.json` релізу і `state.json`. Поза цією текою нічого не
записується. Потім у кожному проєкті:

    cd your-project
    kulman setup --scope repo

Ця команда запускає там `kalka setup` і `plaz setup`. Вони пишуть у теку проєкту `.kulman/` (вікі,
а карту — у `.kulman/map/`), додають конфіги агентів і хук запиту для знайдених агентів, створюють
`.agents/memory/` для нотаток агентів і додають `.kulman/` у `.gitignore`. `--scope repo` пише
закомічені файли для всієї команди; `--scope local` лишає все в межах цієї робочої копії й натомість
вносить створене в `.git/info/exclude`.

Згодом `kulman update` доводить усе до останнього релізу, `kulman setup --remove` скасовує
налаштування проєкту, а `kulman remove` видаляє те, що встановив kulman. Команди, прапорці, коди
виходу й будова релізу: [crates/kulman/README.uk.md](crates/kulman/README.uk.md).

### Перевірка релізу

Реліз перевіряється лише за SHA-256: `install.sh` і `kulman` звіряють кожен архів із сумами з
`SHA256SUMS` і `manifest.json` релізу. Релізи не підписані: суми показують, що завантажений файл —
саме той, що його перелічує реліз, але не те, хто цей реліз зробив. Щоб перевірити вручну, коли
архіви й `SHA256SUMS` лежать в одній теці, виконайте `sha256sum -c --ignore-missing SHA256SUMS` (на
macOS `shasum -a 256 -c --ignore-missing SHA256SUMS`).

## Модулі

**[kulman](crates/kulman/README.uk.md)** встановлює, оновлює й видаляє модулі та запускає їхнє
налаштування в проєкті. Це інсталятор, а не модуль.

**[kalka](crates/kalka/README.uk.md)** компілює проєкт — код, нотатки, документацію — у вікі з
перехресними посиланнями в Markdown і карту символів з діапазонами рядків і віддає її агентам через
MCP: `search` відповідає рядками `path:start-end`, `callers` каже, хто викликає функцію.
`kalka setup` підключає її до Claude Code, pi, Cursor і Codex. У її README — CLI, MCP-сервер,
памʼять агента, формат результату й виміри.

**[plaz](crates/plaz/README.uk.md)** для кожної функції пише сторінку Markdown з блок-схемою
рішень: буквальна умова кожної гілки, хто викликає функцію і куди веде кожен виклик. Є Rust і
Python; інші мови отримують примітку «not covered yet».

**[techlog](crates/techlog/README.md)** — бібліотека: де лежить памʼять агентів — `.agents/memory/`
проєкту й теки, які агенти тримають поза ним, — і хто написав кожну нотатку. Нею користується kalka.
README англійською.

**[wiki-code](crates/wiki-code/README.md)** — бібліотека, спільна для kalka і plaz: обходить дерево
джерел, видобуває функції й типи через tree-sitter, індексує виклики й імпорти. README англійською.

## Вимоги

- Реліз працює на macOS на Apple silicon (`aarch64-apple-darwin`) і на Linux x86_64
  (`x86_64-unknown-linux-musl`). Збірка для Linux статична й працює однаково на системах із glibc і
  з musl. Інших збірок немає; на будь-якій іншій системі, зокрема на Mac з Intel, збирайте з коду.
  Windows не перевірявся.
- `install.sh` потребує `curl`, `tar`, `mktemp` і `sha256sum` або `shasum`.
- Для агентів: MCP-клієнт — Claude Code, pi, Cursor, Codex, Claude Desktop чи інший. Див.
  [вимоги kalka](crates/kalka/README.uk.md#вимоги).

## Збірка з коду

Для збірки потрібен Rust 1.90 чи новіший; для тестів — ще й `jq`.

    git clone https://github.com/bambooplanedev/kulman
    cd kulman
    cargo install --path crates/kalka      # у ~/.cargo/bin
    cargo install --path crates/plaz
    cargo install --path crates/kulman

    cargo test
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

`cargo install` кладе бінарники в `~/.cargo/bin`; `kulman`, зібраний так, однаково встановлює модулі
з опублікованого релізу. Пуш тегу `v*`, що збігається з версією робочого простору, збирає архіви для
macOS і Linux, перевіряє, що вони встановлюються, і створює чернетку GitHub Release з їхніми
`SHA256SUMS`.

## Ліцензія

[MIT](LICENSE)
