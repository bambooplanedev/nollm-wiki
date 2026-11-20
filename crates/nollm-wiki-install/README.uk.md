[English](README.md) · **Українська**

# `nollm-wiki-install`

**Встановлює, оновлює й налаштовує інструменти nollm-wiki — `wiki` і `plaz` — у проєкті.**

`nollm-wiki-install` встановлює, оновлює й налаштовує модулі nollm-wiki: `wiki` (вікі проєкту для агентів,
що пишуть код) і `plaz` (блок-схеми рішень). Це інсталятор, а не модуль. Реліз 0.9.0 — перший, що
його містить; весь набір описано в [README репозиторію](../../README.uk.md).

## Як отримати

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/nollm-wiki/releases/latest/download/install.sh | sh

`install.sh` читає `SHA256SUMS` останнього релізу (і перевіряє його підпис, якщо встановлено
`minisign`, див. [Підписи](#підписи)), завантажує архів `nollm-wiki-install` для цієї машини
(`aarch64-apple-darwin` або `x86_64-unknown-linux-musl`; на будь-якій іншій системі — «no build»),
перевіряє його за цим файлом і кладе бінарник у `~/.nollm-wiki-home/bin/nollm-wiki-install`. Йому потрібні `curl`,
`tar`, `mktemp` і `sha256sum` або `shasum`; без інструмента для контрольних сум він зупиняється, а
не пропускає перевірку. Крім цього бінарника й тек, у яких він лежить, `install.sh` нічого не
записує і не змінює профілів оболонки. За будь-якої помилки він виходить з ненульовим кодом і не
лишає тимчасової теки; перше встановлення також не лишає `nollm-wiki-install` у `bin/` (поверх наявного
встановлення лишається старий), а домашню теку, яку він створив, знову видаляє, якщо вона порожня.
Поверх наявного встановлення (є `state.json`) він наприкінці радить запустити `nollm-wiki-install update`, який
доводить модулі до версії щойно покладеного `nollm-wiki-install`. Збірка для Linux статична й працює однаково на
системах із glibc і з musl.

Те саме вручну: завантажте `nollm-wiki-install-v<version>-<target>.tar.gz`, `SHA256SUMS` і
`SHA256SUMS.minisig` зі сторінки релізу, потім

    minisign -Vm SHA256SUMS -P RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
    grep nollm-wiki-install-v<version>-<target>.tar.gz SHA256SUMS | sha256sum -c -   # на macOS: … | shasum -a 256 -c -
    tar -xzf nollm-wiki-install-v<version>-<target>.tar.gz nollm-wiki-install
    mkdir -p ~/.nollm-wiki-home/bin && mv nollm-wiki-install ~/.nollm-wiki-home/bin/

Додати `~/.nollm-wiki-home/bin` у `PATH` необов'язково, це для набору `nollm-wiki-install` у терміналі (агенти
знаходять `wiki` і без цього; коли `~/.local/bin` у `PATH`, install лінкує туди інструменти). Далі виконайте:

    ~/.nollm-wiki-home/bin/nollm-wiki-install install       # nollm-wiki-install і модулі релізу за замовчуванням
    cd your-project
    nollm-wiki-install setup --scope local         # власний setup кожного встановленого модуля, у цьому проєкті

## Що де лежить

Усе лежить у `~/.nollm-wiki-home` (або в `NOLLM_WIKI_HOME`, якщо задано):

- `bin/nollm-wiki-install` і `bin/<module>` для кожного встановленого модуля;
- `manifest.json` — маніфест установленого релізу;
- `state.json` — установлена версія й модулі, записується останнім (файл, який не є файлом стану,
  записаним nollm-wiki-install, зупиняє кожну команду, і вона його називає: виправте його або видаліть і
  запустіть `nollm-wiki-install install`; файл, який не вдається прочитати, зупиняє її зі шляхом і причиною);
- `.lock` — його тримає команда, що виконується; встановлення, яке не вдалося там, де нічого не
  встановлено, знову видаляє його, якщо саме його створило, і теку теж, якщо саме її створило:
  знайдену домашню теку воно лишає як була.

Батьківська тека цієї теки має існувати; створюється лише сама тека. `bin`, що є симлінком або не є
текою, відхиляється. Жоден профіль оболонки не змінюється.

## Команди

    nollm-wiki-install [status]

Без команди або з `status`: установлена версія й модулі (модуль, бінарника якого немає, позначено
`(missing: run nollm-wiki-install update)`), два рядки `<home>/bin is not on PATH; to type nollm-wiki-install in a terminal,
add it:` і `export PATH="<home>/bin:$PATH"` (з відступом), коли теки там немає (запис у `PATH`, що
веде до `bin/` через символічне посилання, зараховується; так само зараховується посилання в теці з
`PATH` для nollm-wiki-install і кожного встановленого модуля, коли воно перше з такою назвою в `PATH`), і чи є
новіший реліз (`newer release: <X.Y.Z> (nollm-wiki-install update)` або `up to date`). Команда лише читає:
нічого не створюється. Хост релізів має 5 с на зʼєднання й 8 с загалом; якщо їх забракло або сталася
будь-яка помилка, останній рядок каже, до чого не вдалося достукатися і чому.

    nollm-wiki-install install [--modules <MODULES>] [--version <X.Y.Z>] [--yes]

Встановлює nollm-wiki-install і названі модулі (через кому; пробіли навколо назви ігноруються) разом із
модулями, яких вони потребують. Без `--modules` встановлює модулі, які реліз позначає як модулі за
замовчуванням; щоб пропустити якийсь, назвіть решту. `--version` встановлює вказаний реліз замість
останнього. Повторний запуск з іншими модулями додає їх у вже встановленій версії й повертає кожен
бінарник зі списку, якого бракує в `bin/`; встановлений `bin/nollm-wiki-install` тоді лишається як є (`nollm-wiki-install
update` замінює його завжди). Файл `bin/<module>`, якого `state.json` не перелічує, ніколи не
вважається встановленим: його замінює завантажений і перевірений модуль. Коли є новіший реліз,
команда однаково встановлює в уже встановленій версії й каже про це (`newer release X.Y.Z: nollm-wiki-install
update`); коли останній реліз не вдається прочитати, каже про це (`could not check for a newer
release (<base>: <reason>)`). Установлену версію читає, коли вже тримає `.lock`. Якщо встановлена
версія відрізняється від `--version` або новіша за останній реліз, команда нічого не змінює й
виходить з кодом 1: для старішої радить запустити `nollm-wiki-install update`; для новішої — `nollm-wiki-install remove`, а
потім `nollm-wiki-install install --version <X.Y.Z>`. Коли `$HOME/.local/bin` є текою в `PATH`, install і update
кладуть туди посилання на кожен установлений бінарник у `bin/<name>` (`nollm-wiki-install: linked <names> into
<dir>`); усе, що там не є посиланням у `bin/`, лишається як є (`nollm-wiki-install: left <path> as it is (not a
nollm-wiki-install link)`), а `nollm-wiki-install remove` видаляє лише власні посилання. Успішне встановлення закінчується
наступними кроками: ті самі два рядки для `bin/`, якщо її немає в `PATH`, а потім `nollm-wiki-install setup` у
проєкті (поки в `PATH` немає `bin/` і не кожен бінарник у ньому досяжний, команда названа повним
шляхом у `bin/`). Коли робити нічого, команда нічого не записує й натомість каже `already installed:
<names> (<version>)` (і, як вище, про новіший реліз).

    nollm-wiki-install update [--yes]

Оновлює nollm-wiki-install і всі встановлені модулі до останнього релізу — усе або нічого. Реліз, старіший за
встановлений, не встановлюється: нічого не змінюється. Якщо встановлена версія вже остання і всі
файли на місці, команда каже `up to date` і нічого не робить. Кожен оновлений модуль названо з
обома версіями (`updated <name> <old> -> <new>`), модуль, якого потребує новий реліз і якого не
було встановлено, — `installed <name> <version>`, а останній рядок, коли версія змінилася, —
`restart running agent sessions; they keep the old binaries until then`. На тій самій версії
повернуті на місце файли названо `repaired <name> <version>`, без рядка про перезапуск.
Встановлений модуль, якого новий реліз уже не містить, видаляється разом із бінарником і посиланням,
коли решта вже на місці: якщо модуль релізу називає його в `replaces` (перейменування), натомість
встановлюється той модуль, а рядок — `removed <old>: renamed <new>; run "nollm-wiki-install setup
--scope repo" (or local) again in each project set up with <old>`; інакше —
`removed <name>: not in release <version>`. У терміналі про будь-яке видалення спершу питають, і на
встановленій версії теж, а питання закінчується на `; removes <names>?`.

    nollm-wiki-install remove [--modules <MODULES>] [--yes]

Видаляє названі модулі; без `--modules` — усі модулі, сам nollm-wiki-install, його маніфест, стан і lock, потім
`bin/` і домашню теку, якщо вони порожні. Видаляється лише те, що перелічує `state.json`; будь-який
інший файл лишається (без `state.json` не видаляється взагалі нічого, а `bin/nollm-wiki-install`, покладений
`install.sh`, команда називає, щоб ви видалили його самі). Кожен шлях перевіряється до видалення
першого: тека на місці бінарника або будь-що, крім звичайного файлу, на місці `manifest.json` чи
`state.json` зупиняє команду, нічого не видаливши (код виходу 1); так само команду зупиняє модуль,
якого потребує інший встановлений модуль (`nollm-wiki-install: cannot remove <a>: <b> requires it`; назвіть
обидва, щоб видалити обидва). `nollm-wiki-install` — не модуль: `--modules nollm-wiki-install` — помилка використання. У
терміналі команда спершу питає (за замовчуванням — ні); без термінала їй потрібен `--yes`, інакше
вона нічого не змінює й виходить з кодом 2. Коли видаляється все, а `wiki` встановлено, команда ще й
каже, що проєкти, налаштовані через nollm-wiki-install, досі називають `wiki` у конфігах агентів, і спершу в
них треба запустити `nollm-wiki-install setup --scope repo --remove` (або `--scope local`): у терміналі — перед
питанням, інакше — останнім рядком.

    nollm-wiki-install setup [--modules <MODULES>] [--memory | --no-memory] [--agent <LIST>]
                 [--scope repo|local] [--hook | --no-hook] [--remove] [--yes]

Запускає власний `setup` кожного вибраного модуля (`~/.nollm-wiki-home/bin/<module> setup …`) у поточній
теці, у порядку релізу: кожен вибраний модуль запускається, хоч би що повернули попередні. Без
`--modules` налаштовуються всі встановлені модулі. `--scope` іде кожному модулю; `--agent`,
`--hook`/`--no-hook` (хук запиту `wiki`, за замовчуванням вимкнений) і `--memory`/`--no-memory` належать `wiki`, і коли `wiki` не вибрано, це помилка
використання. Памʼять агента ввімкнена, хіба що передано `--no-memory` або її вимикає реліз.
`--remove` скасовує setup кожного модуля. Перед запуском першого модуля бінарник кожного вибраного
модуля має лежати в `bin/` і бути виконуваним; коли не встановлено жодного модуля, налаштовувати
нічого (код виходу 1). Модулі пишуть у власну теку проєкту `.nollm-wiki/`: setup відмовляється (код
виходу 1) працювати там, де ця тека — домашня тека nollm-wiki (`NOLLM_WIKI_HOME`, заданий як
`<project>/.nollm-wiki`). Коли `bin/` немає в `PATH`, модулі запускаються з нею першою у власному `PATH`
(середовище самого nollm-wiki-install і ваша оболонка не змінюються; шлях `bin/` із `:` не може стояти в `PATH`,
тож тоді їхній `PATH` лишається як є). Конфіги агентів, які пише `wiki`, запускають його з
домашньої теки nollm-wiki, тож їм нічого не треба в `PATH`.

## Коли nollm-wiki-install питає, а коли ні

Люди й агенти користуються тими самими командами. Команда питає лише тоді, коли і stdin, і stdout —
термінали, а `--yes` не задано; інакше вона нічого не питає й застосовує значення за замовчуванням
(`install`: те, що реліз має за замовчуванням; `setup`: усі встановлені модулі, памʼять увімкнена).
Значення, що не має замовчування, тоді треба передати прапорцем: `nollm-wiki-install setup` без термінала
потребує `--scope`. Коли nollm-wiki-install не питає, модулі, які він запускає, не отримують вводу, тож теж
нічого не питають. `remove`, чиє питання за замовчуванням має відповідь «ні», без термінала нічого
не робить, якщо не передано `--yes`. Підтвердження nollm-wiki-install питає, коли команда вже тримає `.lock` і
прочитала стан, тож показане — це саме те, що буде зроблено. Назви, введені у відповідь на питання,
зіставляються без огляду на регістр.

## Коди виходу

- `0` — зроблено, або робити нічого.
- `1` — помилка; повідомлення каже, що сталося (для завантаження — що саме й звідки). Невдалий
  `install` чи `update`, що нічого не змінив, закінчується рядком `nothing changed`.
- `2` — помилка використання (невідомий прапорець, порожня чи неправильна назва в `--modules` або
  назва, якої реліз не має, відсутній `--scope`, неправильний `NOLLM_WIKI_RELEASE_URL`, `remove` без
  термінала й без `--yes`).

Кожен модуль, чий setup не вдався, отримує рядок `nollm-wiki-install: <module> setup failed (exit N)` (або
`(signal N)`); решта однаково запускаються. Потім `nollm-wiki-install setup` друкує `nollm-wiki-install: setup finished with
errors: <modules>` і виходить з кодом першого модуля, що не вдався (1, якщо його вбив сигнал).

## Змінні середовища

- `NOLLM_WIKI_HOME` — домашня тека (за замовчуванням `$HOME/.nollm-wiki-home`; порожнє значення — те саме,
  що не задане; шлях із `"`, `$`, зворотним апострофом чи `\` не підтримується рядком запуску
  агентів, який пише `wiki`).
- `NOLLM_WIKI_RELEASE_URL` — де лежать релізи (за замовчуванням
  `https://github.com/bambooplanedev/nollm-wiki/releases`): `https://…` або `file://…`, і більше нічого.

## Один запуск за раз і переривання

Команда, що змінює `~/.nollm-wiki-home`, тримає `.lock`, зокрема поки питає; друга чекає на неї й один
раз каже про це в stderr: `nollm-wiki-install: waiting for another nollm-wiki-install run (<home>/.lock)`. Завантаження
перевіряються й розпаковуються в тимчасову теку в домашній (`.tmp-<pid>`), перш ніж щось у `bin/`
зміниться; потім бінарники перейменовуються в `bin/` (nollm-wiki-install — останнім), далі `manifest.json`, далі
`state.json`. `install.sh` не бере `.lock`, а його тимчасова тека (`.tmp-install.…`) — один із
записів `.tmp-*`, які `nollm-wiki-install install`, `update` і `remove` видаляють у домашній теці з
`state.json`: не запускайте жодної з цих команд, поки працює `install.sh`, інакше `install.sh`
завершиться помилкою.

- Перерване до цього моменту встановлення чи оновлення лишає тільки свою тимчасову теку, яку видаляє
  наступний запуск, що знайде `state.json` (домашню теку без нього не прибирають ніколи).
- Перерване під час перейменування, воно лишає частину бінарників у новій версії, а `state.json`
  досі називає стару. `nollm-wiki-install update` встановлює весь реліз заново.
- Бінарник, який `state.json` перелічує, а в `bin/` його немає (перерваний `remove` чи файл,
  видалений вручну), повертають `nollm-wiki-install update` або `nollm-wiki-install install`, а відсутній `manifest.json` —
  `nollm-wiki-install update`; `nollm-wiki-install setup` натомість каже про це й не запускається.

## Будова релізу

Реліз містить у `<base>/download/v<version>/`:

- `manifest.json` — єдине джерело правди про реліз для nollm-wiki-install;
- `<module>-v<version>-<target>.tar.gz` і `nollm-wiki-install-v<version>-<target>.tar.gz`, у кожному один файл,
  названий за своїм бінарником;
- `SHA256SUMS` — рядки `<sha256>  <file>`, для `install.sh` і для людей;
- `install.sh`;
- `manifest.json.minisig` і `SHA256SUMS.minisig` — підписи цих двох файлів (див.
  [Підписи](#підписи)).

`<base>/latest/download/` віддає ті самі файли останнього релізу. nollm-wiki-install читає маніфест звідти; з
`download/v<version>/` — з `--version`; і з `download/v<installed>/` — для `install` поверх
установленої версії, коли останній реліз новіший або його не вдається прочитати. Кожен архів він
читає з `download/v<version>/`. `install.sh` читає `SHA256SUMS` з `latest/download/`.

`manifest.json`:

```json
{
  "version": "0.9.0",
  "modules": [
    {
      "name": "wiki",
      "summary": "the project wiki for coding agents",
      "default": true,
      "requires": [],
      "options": [{ "name": "memory", "default": true }],
      "archives": {
        "aarch64-apple-darwin": { "file": "wiki-v0.9.0-aarch64-apple-darwin.tar.gz", "sha256": "…" }
      }
    }
  ],
  "nollm-wiki-install": {
    "archives": {
      "aarch64-apple-darwin": { "file": "nollm-wiki-install-v0.9.0-aarch64-apple-darwin.tar.gz", "sha256": "…" }
    }
  }
}
```

Невідомі поля ігноруються. Кожен архів перевіряється за своїм `sha256`, перш ніж його розпакувати.

### Підписи

Опублікований реліз містить 11 файлів: 6 архівів, `manifest.json`, `SHA256SUMS`, `install.sh` і
два відокремлені підписи [minisign](https://jedisct1.github.io/minisign/), `manifest.json.minisig`
і `SHA256SUMS.minisig`. Підписано лише `manifest.json` і `SHA256SUMS`; архіви захищені сумами
SHA-256 усередині них. Ключ релізів — файл [`release/nollm-wiki.pub`](../../release/nollm-wiki.pub) у цьому
репозиторії:

```
untrusted comment: minisign public key 87CC6E990C51AF83
RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
```

У `nollm-wiki-install` ключ вкомпільовано; жоден прапорець чи змінна його не змінює. До кожного `manifest.json`,
який він читає, він завантажує `manifest.json.minisig`, що лежить поруч, і перевіряє його, перш ніж
використати будь-що з маніфесту. Приймається лише форма, яку minisign робить за замовчуванням
(prehashed), а не застаріла (`minisign -S -l`). Через `https://` підпис обовʼязковий, і маніфест без
чинного підпису не використовується: `install` і `update` зупиняються з `nollm-wiki-install: manifest.json:
signature is missing` (хост релізів відповідає 404), `nollm-wiki-install: manifest.json: signature could not be
fetched: <cause>` (будь-яка інша помилка: тайм-аут, TLS, помилка сервера; у дереві `file://` —
`.minisig`, який не вдається прочитати) або `nollm-wiki-install: manifest.json: signature does not verify`, далі
`nothing changed`, код виходу 1. Там, де команда лише дивиться, чи є новіший реліз (`nollm-wiki-install status`
та `install` поверх установленої версії), причину натомість наводить її рядок `could not check for a
newer release (…)`. У дереві `file://` наявний підпис теж мусить пройти перевірку; відсутній
дозволено, і `nollm-wiki-install` один раз каже `signature not checked: local release tree` у stderr: це
повідомлення, а не результат.

`install.sh` містить той самий ключ. Якщо `minisign` є в `PATH`, він завантажує `SHA256SUMS.minisig`
і перевіряє ним `SHA256SUMS` (`minisign -VHm`: як і `nollm-wiki-install`, лише форму prehashed), перш ніж
прочитати з нього бодай рядок; він зупиняється (код виходу 1) з `install.sh: SHA256SUMS: signature
is missing` (через `https://` нічого не завантажено: підпису немає, сервер не відповідає або
оголошує підпис, більший за 4 КіБ) або `install.sh: SHA256SUMS: signature does not verify` (так само
для підпису, більшого за 4 КіБ, який завантажився частково, як із локального дерева чи із сервера,
що не надсилає довжини). Для перевірки потрібен minisign, який знає `-H`; 0.11 і 0.12 його знають, а
з версією, яка не знає, перевірка не проходить і `install.sh` зупиняється. У дереві `file://`
відсутній підпис дозволено, з рядком `install.sh: signature not checked: local release tree`. Без
`minisign` він іде далі лише за SHA-256 і каже про це: `install.sh: signature not checked (minisign
not found); nollm-wiki-install checks it on every install and update`. Коли підпис підтверджено, він друкує
`install.sh: signature checked`. Усі три рядки йдуть у stderr, тож скрипт, що перехоплює вивід, їх
не отримує.

Вручну, коли кожен файл і його `.minisig` лежать в одній теці:

    minisign -Vm manifest.json -P RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
    minisign -Vm SHA256SUMS -P RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr

Підписи зʼявилися раніше, ніж було опубліковано будь-який реліз `nollm-wiki-install`: кожен опублікований
`nollm-wiki-install` їх перевіряє, тож кожен реліз мусить бути підписаний, інакше `install` і `update` його
відхиляють.

Як підписати реліз: пуш тегу `v*` створює чернетку релізу з 9 файлами (робочий процес релізу);
секретного ключа в GitHub немає ніде. Потім власник у терміналі, у робочій копії, де є цей тег,
запускає:

    # з --key <secret key file>, якщо ключ не типовий для minisign
    sh scripts/release-sign.sh <version>

Скрипт перевіряє чернетку (вона мусить бути цього репозиторію, за адресою релізів, якою користується
`nollm-wiki-install`; перед підписом він виводить репозиторій і адресу релізу; рівно 9 файлів; SHA-256 кожного
архіву такий, як у `SHA256SUMS` і `manifest.json`; версію; `install.sh` такий, як на тезі), підписує
обидва файли (minisign питає пароль ключа, по разу на файл), перевіряє обидва підписи ключем
`release/nollm-wiki.pub` (підпис будь-яким іншим ключем зупиняє скрипт, і нічого не вивантажується) і
вивантажує два файли `.minisig` у чернетку; `--replace` підписує наново чернетку, де вони вже є. Сам
він не публікує: наприкінці він виводить команду, яка це робить, `gh release edit v<version>
--draft=false`. Опублікований реліз стає останнім — тим, який беруть `install.sh` і `nollm-wiki-install update`.

Щоб випробувати підписану чернетку до публікації, завантажте її в `$dl` (`gh release download v$v
--dir "$dl"`, де `v` — версія) і розкладіть як дерево релізу в `$out`:

    mkdir -p "$out/download/v$v" "$out/latest/download"
    cp "$dl"/* "$out/download/v$v/"
    cp "$dl"/SHA256SUMS "$dl"/SHA256SUMS.minisig "$dl"/manifest.json "$dl"/manifest.json.minisig \
        "$dl"/install.sh "$out/latest/download/"

Тоді з `NOLLM_WIKI_RELEASE_URL=file://$out` `install.sh` і `nollm-wiki-install` встановлюють кандидата й
перевіряють його підписи. Не запускайте `release-assemble.sh` на завантажених файлах: він
переписує обидва підписані файли.

Обмеження:

- Один ключ, заміни ключа поки немає. Новий ключ може дійти до користувачів лише в релізі,
  підписаному старим, тож якщо загубити секретний ключ (або його пароль), зробити реліз, який
  прийме `nollm-wiki-install`, уже не вдасться: тримайте його резервну копію.
- Підпис показує, що файли зробив власник, а не що вони найновіші. Той, хто контролює хост релізів,
  може видати старіший підписаний реліз за останній: `nollm-wiki-install update` відмовляється повертатися до
  старішої версії, але нове встановлення його прийме.
- Сам `install.sh` надходить через HTTPS і не підписаний: хто контролює місце, звідки його
  завантажують, той контролює цей перший крок, з `minisign` чи без нього. Підписи захищають те,
  що `install.sh` і `nollm-wiki-install` завантажують після нього. Щоб не довіряти жодному хосту завантаження,
  перевіряйте вручну (див. [Як отримати](#як-отримати)).
