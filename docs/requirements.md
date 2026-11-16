# yumekawaOS 設計要件書

## 1. 文書概要

### 1.1 プロジェクト名

**yumekawaOS**

### 1.2 概要

yumekawaOSは、x86_64
PCを対象とした、Rust製の独立したGUIオペレーティングシステムである。

ゆめかわいい世界観を基調とした独自のデスクトップ環境を提供し、ウィンドウ管理、ファイル管理、メモ、設定、ターミナル、Webブラウザなどの基本機能を統合する。

開発時にはQEMU上で動作させることを基本とし、最終的には**ブート可能なISOイメージを生成し、そのISOをUSBメモリへ書き込んで実機PCから起動できる構成**を目標とする。

### 1.3 本文書の位置付け

本書は、yumekawaOSを実装するための本番向け設計要件を定義する。

生成AI等を利用して実装する場合も、本書に記載された機能要件、アーキテクチャ、制約、MVP、受け入れ条件を優先する。

------------------------------------------------------------------------

# 2. プロダクト目標

## 2.1 最終目標

以下を満たす独立したOSを構築する。

-   x86_64 PCで動作する
-   UEFIから起動できる
-   ブート可能なISOイメージを生成できる
-   生成したISOをUSBメモリへ書き込み、実機PCから起動できる
-   QEMUからISOを直接起動できる
-   GUIデスクトップを持つ
-   マウスとキーボードで操作できる
-   複数アプリケーションを同時に実行できる
-   ファイルを永続的に保存できる
-   ネットワークへ接続できる
-   Webブラウザを実行できる
-   yumekawaOS独自の一貫したUI/UXを持つ

## 2.2 プロダクトコンセプト

「実際に起動して使える、小規模だが独立したデスクトップOS」を目指す。

単なるカーネルデモではなく、OSとして以下の一連の操作が成立することを重視する。

``` text
Boot
 ↓
Desktop
 ↓
Application launch
 ↓
Window operation
 ↓
Input
 ↓
File operation
 ↓
Network communication
 ↓
Web browsing
 ↓
Shutdown / reboot
```

------------------------------------------------------------------------

# 3. 開発方針

## 3.1 基本方針

-   実際に動作する機能を優先する
-   モジュール単位で実装する
-   QEMU上で動作確認してから実機へ展開する
-   ハードウェア依存部分を可能な限り分離する
-   Rustの型システムと所有権を最大限利用する
-   `unsafe` は低レイヤーに限定する
-   既存の安定したライブラリやブートローダを必要に応じて利用する
-   暗号方式や複雑なプロトコルを独自実装しない
-   実装できない機能をモックで「実装済み」と扱わない
-   機能追加によって既存機能を破壊しない

## 3.2 優先順位

優先順位は以下とする。

1.  ブート
2.  カーネルの安定動作
3.  メモリ管理
4.  割り込み
5.  入力
6.  描画
7.  GUI
8.  タスク管理
9.  ファイルシステム
10. システムコール
11. ネットワーク
12. アプリケーション
13. ブラウザ
14. サウンド
15. 高度な機能

------------------------------------------------------------------------

# 4. 対象環境

## 4.1 CPU

-   x86_64
-   64bit CPU
-   Long Modeを利用する

ARM、RISC-V等は初期対象外とする。

## 4.2 Firmware

初期ターゲットは**UEFI**とする。

BIOS/Legacy Bootは初期MVPの必須要件としない。

## 4.3 開発環境

開発時の主要実行環境はQEMUとする。

想定環境：

``` text
Host OS
 └─ QEMU
     └─ yumekawaOS
```

Host OSはWindowsまたはLinuxを想定する。

## 4.4 実機

実機については、以下を初期対応範囲とする。

-   x86_64
-   UEFI
-   64bit CPU
-   4GB以上のRAMを推奨
-   UEFI framebufferを利用可能
-   USBブート可能

すべてのPC、GPU、ネットワークデバイス、ストレージデバイスへの対応は要求しない。

------------------------------------------------------------------------

# 5. 技術スタック

## 5.1 言語

主要実装言語：

``` text
Rust
```

必要に応じて以下を利用する。

-   Assembly
-   linker script
-   build script
-   shell script

## 5.2 アーキテクチャ

``` text
Host OS
  ↓
QEMU / Real Hardware
  ↓
UEFI
  ↓
Bootloader
  ↓
yumekawaOS Kernel
  ↓
System Services
  ↓
GUI / Applications
```

## 5.3 ブートローダ

既存のRust対応ブートローダを利用する。

候補：

-   `bootloader`
-   Limine
-   その他、x86_64 UEFIに対応した安定したブートローダ

ブートローダそのものを独自実装することはMVPの必須要件としない。

------------------------------------------------------------------------

# 6. ISO / ブート要件

## 6.1 ISOイメージ

ビルドシステムは、ブート可能なISOイメージを生成できなければならない。

出力例：

``` text
build/
└── yumekawaOS.iso
```

## 6.2 ISOからの起動

ISOは以下の環境で起動可能であること。

### QEMU

``` text
QEMU
 ↓
yumekawaOS.iso
 ↓
UEFI
 ↓
Bootloader
 ↓
Kernel
```

### 実機

``` text
yumekawaOS.iso
 ↓
USBメモリへ書き込み
 ↓
PCのUEFI Boot Menu
 ↓
USB
 ↓
Bootloader
 ↓
yumekawaOS
```

## 6.3 ISOの要件

-   x86_64対応
-   UEFI起動可能
-   必要なEFI Bootloaderを含む
-   カーネルを含む
-   必要な初期RAMディスクまたは初期ファイルを含められる
-   ビルドごとに再生成可能
-   再現可能なビルド手順を用意する

## 6.4 USBブート

USBブートとは、USBデバイスからyumekawaOSのISOイメージを起動することを指す。

**OS内部でUSBメモリやUSBデバイスを汎用的に利用できることとは区別する。**

MVPでは、USBブートのためだけにUSBホストコントローラドライバを実装する必要はない。

------------------------------------------------------------------------

# 7. カーネル

## 7.1 基本要件

カーネルは以下の機能を提供する。

-   CPU初期化
-   メモリ管理
-   割り込み管理
-   タスク管理
-   システムコール
-   デバイス管理
-   ファイルシステムアクセス
-   ネットワーク機能への基盤提供
-   GUIへの基盤提供

## 7.2 カーネル構成

以下のようなモジュール構成を基本とする。

``` text
kernel/
├── arch/
├── boot/
├── memory/
├── interrupts/
├── scheduler/
├── task/
├── syscall/
├── drivers/
├── fs/
├── net/
├── graphics/
└── kernel.rs
```

------------------------------------------------------------------------

# 8. メモリ管理

## 8.1 必須機能

-   物理メモリ管理
-   ページング
-   仮想メモリ
-   カーネルヒープ
-   ヒープアロケータ
-   メモリ領域の保護

## 8.2 メモリ保護

ユーザー空間とカーネル空間を分離する。

``` text
User Space
    │
    │ System Call
    ▼
Kernel Space
```

ユーザーアプリケーションが任意のカーネルメモリへ直接アクセスできないようにする。

------------------------------------------------------------------------

# 9. 割り込み・例外

## 9.1 必須

-   IDT
-   Timer Interrupt
-   Keyboard Interrupt
-   Mouse Interrupt
-   Page Fault
-   General Protection Fault
-   Double Fault

## 9.2 エラー処理

致命的例外発生時には、

-   エラー情報をログへ出力
-   可能な場合は該当タスクのみ停止
-   回復不能な場合はKernel Panic UIを表示

する。

------------------------------------------------------------------------

# 10. タスク・プロセス管理

## 10.1 タスク

OSは複数のタスクを管理できること。

例：

``` text
Kernel Task
Browser Task
Memo Task
Files Task
Terminal Task
Desktop Task
```

## 10.2 マルチタスク

MVPでは**プリエンプティブ・マルチタスク**を必須とする。

Timer Interruptを利用してタスクを切り替える。

## 10.3 スケジューラ

初期スケジューラはRound Robin方式を基本とする。

将来的な優先度制御を妨げない設計にする。

------------------------------------------------------------------------

# 11. システムコール

ユーザー空間アプリケーションからカーネル機能へアクセスするため、システムコールAPIを提供する。

最低限、以下を想定する。

``` text
process/task
memory
file
time
input
graphics
network
```

APIは内部実装から分離する。

------------------------------------------------------------------------

# 12. 入力デバイス

## 12.1 キーボード

MVPで必須。

必要な機能：

-   英数字
-   記号
-   Shift
-   Ctrl
-   Alt
-   Enter
-   Backspace
-   Tab
-   Esc
-   矢印キー
-   ファンクションキーの基本対応

日本語IMEはMVP必須ではない。

## 12.2 マウス

MVPで必須。

必要な機能：

-   X/Y移動
-   左クリック
-   右クリック
-   ホイール
-   ドラッグ

------------------------------------------------------------------------

# 13. グラフィックス

## 13.1 基本方式

初期実装ではCPUによるソフトウェアレンダリングを使用する。

GPUアクセラレーションはMVP対象外。

## 13.2 Framebuffer

UEFI framebufferまたは同等の線形Framebufferを利用する。

最低限、

-   Pixel描画
-   Rectangle
-   Line
-   Image
-   Text

を描画できるAPIを提供する。

------------------------------------------------------------------------

# 14. フォント

## 14.1 必須

GUIでテキストを描画できること。

## 14.2 文字コード

内部テキスト処理はUTF-8を基本とする。

## 14.3 日本語

日本語表示は可能な限り対応する。

ただし、MVPでは日本語入力を必須としない。

------------------------------------------------------------------------

# 15. GUIシステム

## 15.1 基本構成

``` text
Desktop
 ├── Dock
 ├── Window Manager
 │    ├── Browser
 │    ├── Memo
 │    ├── Files
 │    ├── Settings
 │    └── Terminal
 └── Notification
```

## 15.2 ウィンドウ

各ウィンドウは最低限以下を持つ。

-   タイトルバー
-   コンテンツ領域
-   閉じるボタン
-   最小化
-   最大化
-   フォーカス状態
-   Z-order

## 15.3 ウィンドウ操作

MVPで必須：

-   移動
-   リサイズ
-   最小化
-   最大化
-   復元
-   閉じる
-   フォーカス切り替え
-   重なり順変更

------------------------------------------------------------------------

# 16. yumekawa UI/UX

## 16.1 デザインコンセプト

UI全体を「ゆめかわいいデジタル世界」として統一する。

## 16.2 ビジュアル要素

使用候補：

-   パステルピンク
-   ラベンダー
-   ミント
-   ホワイト
-   星
-   ハート
-   月
-   雲
-   リボン
-   キラキラ
-   丸みを帯びたUI

## 16.3 UI原則

-   全アプリで共通するデザイン言語を使用する
-   ウィンドウ形状を統一する
-   ボタン、入力欄、メニューの挙動を統一する
-   過度な装飾によって操作性を損なわない
-   UI状態を明確に区別する

## 16.4 ビジュアルリファレンス

UI/UXの方向性として、ゲーム作品等のポップでデジタルなUI表現を参考にする。

ただし、特定作品のUI、キャラクター、ロゴ、素材、画面構成をそのまま複製しない。

------------------------------------------------------------------------

# 17. Desktop

デスクトップには以下を配置する。

-   壁紙
-   Dock
-   アプリケーションアイコン
-   時刻
-   システム状態
-   通知

Dockから主要アプリケーションを起動できること。

------------------------------------------------------------------------

# 18. アプリケーション

MVPでは以下を実装する。

## 18.1 Browser

Webページを閲覧するためのアプリケーション。

## 18.2 Memo

テキストを編集・保存できるアプリケーション。

最低限：

-   新規作成
-   編集
-   保存
-   開く
-   名前を付けて保存

## 18.3 Files

ファイル管理アプリケーション。

最低限：

-   ディレクトリ表示
-   ファイル表示
-   ファイル作成
-   ファイル削除
-   ファイル名変更
-   ディレクトリ作成
-   ファイルを開く

## 18.4 Settings

最低限：

-   ディスプレイ設定
-   音量設定
-   ネットワーク設定
-   UI設定
-   システム情報

## 18.5 Terminal

最低限のコマンドラインインターフェース。

例：

``` text
help
ls
cd
pwd
cat
mkdir
touch
rm
cp
mv
clear
reboot
shutdown
```

------------------------------------------------------------------------

# 19. ファイルシステム

## 19.1 要件

永続ストレージへファイルを保存できること。

必要な機能：

-   ファイル
-   ディレクトリ
-   作成
-   読み込み
-   書き込み
-   削除
-   リネーム
-   メタデータ

## 19.2 初期実装

MVPでは既存の単純なファイルシステムを利用してもよい。

例：

-   FAT32
-   ext2
-   その他、実装負荷と互換性を考慮した方式

独自ファイルシステムを作ることは必須ではない。

------------------------------------------------------------------------

# 20. ストレージ

初期実装では、仮想ディスクまたは対応可能な実機ストレージを対象とする。

優先順位：

1.  QEMU仮想ディスク
2.  USBブート
3.  実機ストレージ
4.  NVMe等

NVMeの完全対応はMVP後とする。

------------------------------------------------------------------------

# 21. ネットワーク

## 21.1 MVP

以下を実装対象とする。

``` text
Ethernet
 ↓
ARP
 ↓
IPv4
 ↓
ICMP
 ↓
UDP / TCP
 ↓
DNS
 ↓
HTTP
```

## 21.2 QEMU

QEMUのUser
Networking等を利用して、開発時にインターネットへ接続できる構成を用意する。

## 21.3 NIC

最初に対応する仮想NICを1種類に固定する。

例：

``` text
e1000
```

実機NICの網羅的対応はMVP対象外。

------------------------------------------------------------------------

# 22. DNS

DNS名前解決を実装する。

例：

``` text
example.com
    ↓
DNS
    ↓
IP address
```

------------------------------------------------------------------------

# 23. HTTP / HTTPS

## 23.1 HTTP

HTTP/1.1を基本とする。

最低限：

-   GET
-   Response Status
-   Headers
-   Body
-   Content-Length
-   Chunked Transferへの将来的対応

## 23.2 HTTPS

TLSは独自実装しない。

既存の暗号/TLSライブラリを利用する。

`no_std`環境との互換性などの理由で初期段階のHTTPS実装が困難な場合、HTTPを先に実装し、HTTPSを後続フェーズへ分離する。

------------------------------------------------------------------------

# 24. Browser

## 24.1 目的

yumekawaOS上で実際のWebページを閲覧できること。

## 24.2 MVP範囲

MVPでは以下を実装する。

``` text
URL入力
 ↓
DNS
 ↓
TCP
 ↓
HTTP / HTTPS
 ↓
HTML Parser
 ↓
Document Tree
 ↓
Layout
 ↓
Text/Image Rendering
 ↓
画面表示
```

## 24.3 HTML

最低限対応：

-   html
-   head
-   body
-   title
-   h1-h6
-   p
-   a
-   img
-   ul
-   ol
-   li
-   div
-   span
-   br

## 24.4 CSS

MVPでは限定的なCSSを実装してよい。

例：

-   color
-   background
-   font-size
-   width
-   height
-   margin
-   padding
-   border
-   text-align

完全なCSS仕様への対応は要求しない。

## 24.5 JavaScript

完全なJavaScriptエンジンはMVP対象外。

将来的に必要となった場合は、独立したJS/WASMランタイムの導入を検討する。

------------------------------------------------------------------------

# 25. Browser UI

最低限：

``` text
┌────────────────────────────────────────────┐
│ ←  →  ⟳   https://example.com              │
├────────────────────────────────────────────┤
│                                            │
│              Web Page                     │
│                                            │
└────────────────────────────────────────────┘
```

必要な機能：

-   URL入力
-   ページ移動
-   戻る
-   進む
-   再読み込み
-   スクロール
-   リンククリック

------------------------------------------------------------------------

# 26. マルチタスクとアプリケーション

複数アプリケーションを同時に起動できること。

例：

``` text
Browser
Memo
Files
Terminal
Settings
```

を同時に開いた状態で、

-   ウィンドウを切り替える
-   別アプリを操作する
-   ファイルを保存する
-   ブラウザで通信する

ことができる。

------------------------------------------------------------------------

# 27. 時刻

システム時刻を取得できること。

GUI上に時刻を表示する。

将来的に：

-   タイマー
-   日付
-   カレンダー
-   ファイルのタイムスタンプ

へ利用できる設計とする。

------------------------------------------------------------------------

# 28. サウンド

## 28.1 MVP

サウンドはMVPの必須機能ではないが、アーキテクチャ上は将来的な追加を妨げない設計にする。

## 28.2 将来機能

-   システムサウンド
-   UIクリック音
-   通知音
-   音楽再生
-   音量制御
-   オーディオデバイス管理

GPUと同様に、複雑な実機オーディオドライバはMVP対象外。

------------------------------------------------------------------------

# 29. 通知

OS標準の通知システムを用意する。

例：

``` text
┌───────────────────────────┐
│ ♡ yumekawaOS             │
│ File saved successfully. │
└───────────────────────────┘
```

通知はアプリケーションから共通API経由で発行できるようにする。

------------------------------------------------------------------------

# 30. エラー処理

## 30.1 ユーザー向けエラー

システムエラーを直接ログ文字列だけで表示せず、可能な場合はGUIで説明する。

例：

``` text
┌────────────────────────────────┐
│        Something went wrong    │
│                                │
│  The file could not be opened. │
│                                │
│          [ OK ]                │
└────────────────────────────────┘
```

## 30.2 Kernel Panic

カーネルが回復不能な状態になった場合、Kernel Panic画面を表示する。

最低限：

-   エラー種別
-   エラーコード
-   CPU情報
-   命令ポインタ
-   必要なログ

を表示する。

------------------------------------------------------------------------

# 31. セキュリティ

MVPでは以下を重視する。

-   User/Kernel分離
-   仮想メモリによる保護
-   権限の分離
-   入力値検証
-   ファイルパス検証
-   バッファ境界の安全性
-   ネットワーク入力の検証

暗号アルゴリズムを独自実装しない。

------------------------------------------------------------------------

# 32. Rust安全性方針

## 32.1 `unsafe`

`unsafe`は必要最低限に限定する。

主な利用箇所：

-   CPU命令
-   MMIO
-   Port I/O
-   Interrupt Handler
-   Context Switch
-   Page Table
-   Bootloader interface
-   Memory mapped device

## 32.2 安全境界

低レイヤーの`unsafe`コードを安全なRust
APIでラップし、上位層では可能な限りsafe Rustのみを使用する。

------------------------------------------------------------------------

# 33. ディレクトリ構成

例：

``` text
yumekawaOS/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── build.rs
│
├── kernel/
│   ├── src/
│   │   ├── main.rs
│   │   ├── memory/
│   │   ├── interrupts/
│   │   ├── scheduler/
│   │   ├── task/
│   │   ├── syscall/
│   │   ├── drivers/
│   │   ├── fs/
│   │   ├── net/
│   │   ├── graphics/
│   │   └── arch/
│   │
│   └── linker.ld
│
├── user/
│   ├── browser/
│   ├── memo/
│   ├── files/
│   ├── settings/
│   └── terminal/
│
├── gui/
│   ├── compositor/
│   ├── window_manager/
│   ├── widgets/
│   ├── theme/
│   └── desktop/
│
├── assets/
│   ├── fonts/
│   ├── icons/
│   ├── wallpapers/
│   └── sounds/
│
├── scripts/
│   ├── build.sh
│   ├── run.sh
│   └── make_iso.sh
│
└── docs/
    ├── architecture.md
    ├── syscall.md
    ├── filesystem.md
    ├── network.md
    └── browser.md
```

実装状況や規模に応じて構成は変更してよいが、カーネル、GUI、アプリケーション、アセット、ビルド処理を明確に分離する。

------------------------------------------------------------------------

# 34. 開発フェーズ

## Phase 1: Boot

-   Rustプロジェクト作成
-   x86_64設定
-   UEFI対応
-   Bootloader導入
-   Kernel起動
-   QEMU起動
-   ISO生成

### 完了条件

``` text
yumekawaOS.iso
 ↓
QEMU
 ↓
UEFI
 ↓
Kernel
 ↓
画面表示
```

が成立する。

------------------------------------------------------------------------

## Phase 2: Memory / Interrupt

-   Page Table
-   Heap
-   Frame Allocator
-   IDT
-   Timer
-   Exception
-   Keyboard

------------------------------------------------------------------------

## Phase 3: Graphics / Input

-   Framebuffer
-   Pixel Renderer
-   Font Renderer
-   Keyboard
-   Mouse

------------------------------------------------------------------------

## Phase 4: GUI

-   Desktop
-   Window
-   Window Manager
-   Dock
-   Widgets
-   Theme
-   Notification

------------------------------------------------------------------------

## Phase 5: Multitasking

-   Task
-   Context Switch
-   Scheduler
-   Timer-based preemption
-   User process

------------------------------------------------------------------------

## Phase 6: Filesystem

-   Block Device
-   Filesystem
-   File API
-   Directory API
-   Persistent storage

------------------------------------------------------------------------

## Phase 7: User Space / System Calls

-   User/Kernel separation
-   Syscall
-   Process management
-   File access
-   GUI API

------------------------------------------------------------------------

## Phase 8: Network

-   NIC
-   Ethernet
-   ARP
-   IPv4
-   ICMP
-   UDP
-   TCP
-   DNS

------------------------------------------------------------------------

## Phase 9: Browser

-   HTTP
-   HTTPS
-   HTML parser
-   DOM-like structure
-   Layout
-   Text rendering
-   Image rendering
-   Basic CSS
-   Browser UI

------------------------------------------------------------------------

## Phase 10: Applications

-   Memo
-   Files
-   Settings
-   Terminal
-   Browser integration
-   Application lifecycle

------------------------------------------------------------------------

## Phase 11: ISO / Real Hardware

-   ISO build stabilization
-   UEFI boot testing
-   USB boot testing
-   Multiple PC testing
-   Hardware compatibility documentation

------------------------------------------------------------------------

# 35. MVP

## 35.1 MVPの定義

yumekawaOSのMVPは、

> **x86_64 UEFI
> PCでブート可能なISOを生成し、QEMUまたはUSBメモリ経由の実機PCから起動でき、GUIデスクトップ上で複数アプリケーションを操作し、ファイル操作とネットワーク通信を行えるOS**

とする。

## 35.2 MVP必須機能

### Boot

-   [x] x86_64
-   [x] UEFI
-   [x] Bootloader
-   [x] ISO生成
-   [x] QEMU ISO Boot
-   [x] USBメモリ経由の実機Boot

### Kernel

-   [x] Memory Management
-   [x] Interrupt
-   [x] Exception
-   [x] Timer
-   [x] Scheduler
-   [x] Task
-   [x] System Call

### Input

-   [x] Keyboard
-   [x] Mouse

### Graphics

-   [x] Framebuffer
-   [x] Font
-   [x] 2D Renderer

### GUI

-   [x] Desktop
-   [x] Window Manager
-   [x] Dock
-   [x] Window operation
-   [x] yumekawa Theme

### Storage

-   [x] Filesystem
-   [x] File read/write
-   [x] Directory
-   [x] Persistent storage

### Network

-   [x] Ethernet
-   [x] IPv4
-   [x] TCP/UDP
-   [x] DNS
-   [x] HTTP

### Applications

-   [x] Browser
-   [x] Memo
-   [x] Files
-   [x] Settings
-   [x] Terminal

### Browser

-   [x] URL input
-   [x] HTTP request
-   [x] HTML parsing
-   [x] Text rendering
-   [x] Basic links
-   [x] Basic images
-   [ ] Full JavaScript
-   [ ] Full CSS

------------------------------------------------------------------------

# 36. MVPで対象外とする機能

以下はMVP完成後の拡張対象とする。

-   GPU acceleration
-   3D graphics
-   Bluetooth
-   Wi-Fi
-   Thunderbolt
-   USB device ecosystem
-   USB audio
-   NVMe完全対応
-   高度な電源管理
-   マルチコアCPU
-   完全なPOSIX互換
-   Linux binary compatibility
-   完全なHTML5
-   完全なCSS
-   完全なJavaScript
-   WebAssembly
-   高度なオーディオ
-   GPUドライバ
-   全PCへの互換性
-   ARM
-   RISC-V

------------------------------------------------------------------------

# 37. 非機能要件

## 37.1 安定性

-   Kernel Panicを可能な限り減らす
-   メモリ破壊を防止する
-   タスク間のメモリ干渉を防止する
-   アプリケーション異常終了がOS全体のクラッシュにつながらない設計とする

## 37.2 保守性

-   モジュール間の依存関係を明確にする
-   APIを定義する
-   ハードウェア依存部分を分離する
-   重要なunsafeコードをコメントする
-   デバッグログを用意する

## 37.3 拡張性

将来的に以下を追加できる構造とする。

-   GPU
-   Audio
-   Wi-Fi
-   Bluetooth
-   USB
-   JavaScript
-   WebAssembly
-   マルチコア
-   より高度なFilesystem

------------------------------------------------------------------------

# 38. デバッグ

以下を利用する。

-   QEMU serial output
-   Kernel log
-   QEMU monitor
-   GDB
-   panic log
-   debug build

ログレベルを用意する。

例：

``` text
TRACE
DEBUG
INFO
WARN
ERROR
FATAL
```

------------------------------------------------------------------------

# 39. テスト

## 39.1 Boot Test

``` text
ISO生成
 ↓
QEMU起動
 ↓
Kernel起動
 ↓
Desktop表示
```

## 39.2 GUI Test

-   ウィンドウを開く
-   ウィンドウを閉じる
-   移動
-   リサイズ
-   最小化
-   最大化
-   フォーカス変更

## 39.3 Filesystem Test

-   ファイル作成
-   書き込み
-   読み込み
-   削除
-   リネーム
-   ディレクトリ作成

## 39.4 Multitasking Test

Browser、Memo、Files、Terminalを同時に起動し、相互に操作できること。

## 39.5 Network Test

``` text
DNS
 ↓
IP取得
 ↓
TCP
 ↓
HTTP
 ↓
HTML取得
```

## 39.6 Browser Test

外部Webページまたはテスト用Webページを表示できること。

## 39.7 ISO Test

生成されたISOが、

1.  QEMUで起動する
2.  UEFI環境で起動する
3.  USBメモリへ書き込んで実機起動できる

こと。

------------------------------------------------------------------------

# 40. パフォーマンス目標

MVPでは絶対的な高性能よりも安定動作を優先する。

目安：

-   Desktop起動後の操作に明確な遅延がない
-   ウィンドウ操作が継続的に応答する
-   複数アプリケーションを起動してもOSが停止しない
-   Browserの基本ページ表示が完了する
-   メモリリークを長時間放置しない

GPUアクセラレーションなしでも基本GUIが実用的な応答性を持つことを目標とする。

------------------------------------------------------------------------

# 41. 実機対応ポリシー

「実機対応」は「すべてのPCで動作する」ことを意味しない。

初期対応環境を明確に定義する。

## 必須

-   x86_64
-   UEFI
-   Framebuffer
-   USB Boot

## 優先

-   USB Keyboard
-   USB Mouse
-   Ethernet
-   SATA

## 後回し

-   Wi-Fi
-   Bluetooth
-   NVMe
-   NVIDIA GPU
-   AMD GPU
-   USB Audio
-   Thunderbolt

------------------------------------------------------------------------

# 42. ビルド成果物

ビルドによって以下を生成できること。

``` text
build/
├── kernel
├── bootloader
└── yumekawaOS.iso
```

ISOは単独で配布可能な成果物とする。

------------------------------------------------------------------------

# 43. 開発AIへの実装指示

生成AIを利用して実装する場合、以下を遵守する。

## 43.1 実装方針

-   要件を勝手に縮小しない
-   未実装機能を実装済みとして扱わない
-   ダミー処理でOS機能を偽装しない
-   各機能を実際に動作させる
-   コンパイル可能なコードを優先する
-   既存コードとの整合性を確認する
-   APIを勝手に破壊しない
-   依存関係を明示する

## 43.2 変更単位

大規模な変更を一度に行わず、

``` text
1. 設計
2. 実装
3. Build
4. Test
5. 修正
6. 次の機能
```

の順に進める。

## 43.3 エラー発生時

コンパイルエラーやKernel
Panicが発生した場合、エラーを隠さず原因を特定する。

ログを削除して問題を見えなくすることは禁止する。

## 43.4 外部ライブラリ

外部ライブラリを使用する場合：

-   ライブラリ名
-   バージョン
-   用途
-   `no_std`対応状況
-   ライセンス
-   OS環境との互換性

を記録する。

------------------------------------------------------------------------

# 44. 完成条件

yumekawaOS v1.0候補は、最低限以下を満たすこと。

``` text
[Power On]
     ↓
[UEFI]
     ↓
[yumekawaOS ISO / Bootloader]
     ↓
[Kernel]
     ↓
[Memory / Interrupt / Scheduler]
     ↓
[GUI]
     ↓
[Desktop]
     ├── Browser
     ├── Memo
     ├── Files
     ├── Settings
     └── Terminal
     ↓
[Filesystem]
     ↓
[Network]
     ↓
[Web]
```

さらに、

-   QEMUでISOから起動できる
-   USBメモリへISOを書き込んで実機起動できる
-   GUIをマウスとキーボードで操作できる
-   複数アプリケーションを同時に実行できる
-   ファイルを保存できる
-   ネットワーク通信できる
-   基本的なWebページを表示できる
-   yumekawaOSとして一貫したUIを持つ

ことを完成条件とする。

------------------------------------------------------------------------

# 45. 将来拡張

v1.0以降では、必要に応じて以下を追加する。

## Hardware

-   GPU acceleration
-   NVMe
-   Wi-Fi
-   Bluetooth
-   USB
-   Audio
-   ACPI
-   Power Management

## Software

-   Advanced Browser
-   JavaScript
-   WebAssembly
-   Package Manager
-   User Account
-   Permissions
-   Advanced Filesystem

## UI

-   Theme system
-   Widgets
-   Animations
-   Desktop customization
-   Notification Center
-   System Settings expansion

## System

-   SMP / Multi-core
-   Process isolation
-   Advanced scheduler
-   Virtual memory optimization
-   Driver framework

------------------------------------------------------------------------

# 46. 最終プロダクト像

yumekawaOSは、単なるカーネルデモやGUIモックではなく、

**「ISOとして配布でき、USBメモリから実機PCへ起動でき、ゆめかわいいGUIデスクトップ上で複数のアプリケーションを実際に動かせる独立OS」**

を最終的なプロダクト像とする。

開発時はQEMUを主要な検証環境として利用し、QEMU上での安定動作を確立した後、ISOを実機へ展開する。

最終的な成果物は、

``` text
yumekawaOS.iso
```

として扱えるものとする。
