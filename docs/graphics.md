# 2D 描画ライブラリ (Phase 3a: `yk-gfx`)

要件: [requirements.md](requirements.md) §13 (グラフィックス), §14 (フォント), §16 (ゆめかわ UI), §24 (将来のブラウザ), §31/§32 (安全性)。
実装: `gui/gfx/` (パッケージ名 `yk-gfx`)。フォント: `assets/fonts/`。

`yk-gfx` は **`no_std` + `alloc`、ハードウェア非依存、`#![forbid(unsafe_code)]`** の 2D 描画クレート。
カーネル (バックバッファ)、将来のコンポジタ・ウィジェット・ブラウザ (`<img>`、テキスト) が共通に使う。
framebuffer への書き込みは行わず、**メモリ上のピクセルバッファに描く** (framebuffer への転送は統合側。§9)。

```text
gui/gfx/
├── Cargo.toml
├── src/
│   ├── lib.rs        公開 API の再エクスポート
│   ├── color.rs      Color (ARGB8888)、blend_over (source-over)、ゆめかわパレット
│   ├── rect.rs       Rect (交差/合併/包含/clamp)、DamageList (dirty rect 蓄積)
│   ├── image.rs      Image (= Surface: 所有バッファ)、ImageView (借用の読み取り専用ビュー)
│   ├── canvas.rs     Canvas (借用バッファへの描画面)、塗り・矩形・マスク・画像転送・拡縮
│   ├── shapes.rs     線・太線・角丸矩形・円/楕円・グラデーション (Canvas の impl の続き)
│   ├── png.rs        PNG デコード (zune-png)
│   ├── text.rs       フォント読み込み・メトリクス・レイアウト・グリフキャッシュ・描画 (ab_glyph)
│   ├── bitmap.rs     ヒープ不要の 8x8 ビットマップフォント描画
│   └── present.rs    ARGB8888 → framebuffer 画素形式の変換 (行単位)
├── tests/            統合テスト (ホストで実行) と PNG フィクスチャ (tests/data/)
├── examples/         showcase.rs (目視確認用 PNG 出力)、bench.rs (ベンチ)
└── tools/softfloat-probe/   soft-float でのリンク確認・速度計測用の使い捨てバイナリ (§8.3)
```

## 1. ピクセル形式と合成

- **内部表現は ARGB8888 / 非プリマルチプライド (straight alpha) の `u32`** (`0xAARRGGBB`)。
  リトルエンディアンのメモリ上ではバイト順が `B, G, R, A`。
- `Color(pub u32)`。`Color::argb(a, r, g, b)` / `Color::rgb(r, g, b)` / `a()`, `r()`, `g()`, `b()`、
  `with_alpha`, `mul_alpha` (全体の不透明度の調整)、`lerp(other, t, den)` (全チャネルの線形補間、`den == 0` や
  `t > den` でも安全)。
- **合成は source-over** (`blend_over(dst, src)` / `Color::over`)。整数演算のみ。
  - `src.a == 255` → `src`、`src.a == 0` → `dst` (分岐で即座に返す)。
  - `dst` が不透明 (通常のバックバッファ) → 除算なしの高速経路: `ch = div255(s*a + d*(255-a))`
    (`div255` は丸め付きの `x / 255` を加算とシフトだけで求める)。結果のアルファは 255 のまま。
  - `dst` が半透明 → 一般式 `ao = sa + da*(1-sa)`, `co = (cs*sa + cd*da*(1-sa)) / ao` (チャネルごとに除算)。
    オフスクリーンレイヤーを透明な背景に描く場合のみ使われる。
- 描画関数の `Color` 引数は **すべて source-over で合成される** (`a == 255` は単純な書き込みの高速経路)。
  合成せずに上書きしたいときは `clear` / `clear_rect` / `blit` (Copy)。
- 数値の例 (テストで固定): 白 50% (`a=128`) を黒に重ねる → `0xFF808080`、赤 50% を青に → `(128, 0, 127)`、
  `a=128` の白を `a=128` の黒に → `a=192, rgb=170`。

## 2. 座標とクリッピングの規約 (§31 バッファ境界)

- 座標は **`i32`** (左上が原点、x は右、y は下)、幅・高さは **`u32`**。画面外・負座標にはみ出す図形は正しく切り取られる。
- `Rect { x, y, w, h }` は **半開区間** (`x..x+w`, `y..y+h`)。端点の計算 (`right()`, `bottom()`, 面積) は `i64`/`u64`
  で行うのでオーバーフローしない。`from_ltrb` / `union` / `translate` / `inflate` の結果は `i32` の範囲に飽和する。
- `Canvas` は生成時に `stride >= width` かつ `buf.len() >= stride*(height-1) + width` を **検証** する
  (オーバーフローは `GfxError::TooLarge`)。`Image::new` は画素数の上限 `Image::MAX_PIXELS` (2^28) を持ち、
  メモリ確保は `try_reserve` なので **失敗しても abort せず `Err(OutOfMemory)`** になる。
- すべての描画は **クリップ矩形** (`Canvas::clip()`、常にキャンバス範囲の内側) との交差を `i64` で計算してから
  `usize` に落とし、行スライスを **安全な添字** で取得して書く。`unsafe` が無いので範囲外書き込みは構造的に起きない
  (論理バグがあってもメモリ破壊ではなくパニックになる)。
  - 検証: `guard_band_never_written_under_random_extreme_arguments` (stride 付きの小さなキャンバスを大きなバッファの内側に置き、
    全描画 API に `i32::MIN/MAX`・`u32::MAX` 近傍を含む乱数引数を与えても、キャンバス外のガード領域が 1 画素も変わらない)、
    `text_never_writes_outside_canvas` (テキスト版)。
- `set_clip(rect)` はキャンバス範囲に切り詰められる。`sub_canvas(rect)` は矩形だけを切り出した子キャンバスを返す
  (座標原点は `rect` の左上。ウィジェットの描画に使う)。`reborrow()` は同じバッファを短い寿命で借り直す。
- 線の切り取り: `line` (1px) は主軸方向にクリップ範囲のぶんだけ反復し、副軸位置は閉じた式
  `round(i * |dminor| / |dmajor|)` で求める。そのため **端点が画面外の巨大な線でも時間は画面サイズに比例**
  し、画面内に見えるピクセルは切り取りなしの線と同じになる。

## 3. 使い方 (抜粋)

```rust
use yk_gfx::png::{PngLimits, decode_png};
use yk_gfx::present::PixelLayout;
use yk_gfx::text::{Font, TextAlign, TextRenderer, TextStyle};
use yk_gfx::{BlendMode, Canvas, Color, DamageList, Filter, Image, Rect, palette};

static FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/MPLUSRounded1c-Regular.ttf");

// 1. バックバッファ (所有)。描画は Canvas を借りて行う。
let mut back = Image::new(1280, 800).unwrap();
{
    let mut c = back.canvas();
    c.fill_gradient_v(Rect::new(0, 0, 1280, 800), palette::PASTEL_PINK, palette::LAVENDER);
    c.fill_round_rect(Rect::new(40, 40, 400, 300), 20, Color::argb(235, 255, 255, 255));
    c.stroke_round_rect(Rect::new(40, 40, 400, 300), 20, 2, palette::RIBBON_PINK);
    c.line(60, 100, 420, 100, 1, palette::HOT_PINK);
    c.fill_circle(80, 70, 8, palette::HOT_PINK);
}

// 2. テキスト。フォントは &'static [u8] を渡す (クレートはフォントを内蔵しない)。
let font = Font::new(FONT_REGULAR).unwrap();
let mut text = TextRenderer::new();                 // グリフキャッシュ (既定 2 MiB)
let style = TextStyle::new(20.0, palette::INK);     // サイズ (px)、色
{
    let mut c = back.canvas();
    text.draw_text(&mut c, &font, &style, 60, 60, "ゆめかわ OS");          // 1 行 (y は行ボックスの上端)
    let r = Rect::new(60, 120, 360, 200);                                  // 矩形内に折り返して描く
    text.draw_text_box(&mut c, &font, &style.align(TextAlign::Left), r, "吾輩は猫である。名前はまだ無い。");
}
let m = font.measure_text(20.0, "ゆめかわ OS");      // m.width / m.height / m.lines

// 3. PNG (サイズ上限つき) と画像転送。
let icon = decode_png(png_bytes, &PngLimits::default()).unwrap();
back.canvas().blit_alpha(&icon.view(), 10, 10);
back.canvas().blit_scaled(&icon.view(), icon.bounds(), Rect::new(0, 0, 26, 18),
                          Filter::Bilinear, BlendMode::SourceOver, 255);

// 4. 借用バッファ (外部のメモリ) への描画。
let mut raw = vec![0u32; 64 * 48];
let mut c = Canvas::new(&mut raw, 64, 48, 64).unwrap();     // (buf, width, height, stride)
c.fill_rect(Rect::new(0, 0, 10, 10), Color::WHITE);

// 5. ダメージ管理と framebuffer 形式への変換 (行単位)。
let mut damage = DamageList::new();
damage.add(Rect::new(0, 0, 100, 50));
let layout = PixelLayout::XRGB8888;                          // framebuffer の R/G/B の配置から作る
let mut row = vec![0u32; 1280];
for r in damage.take() {
    let r = r.clamp_to(&back.bounds());
    for y in r.y..r.bottom() as i32 {
        let src = &back.view().row(y as u32).unwrap()[r.x as usize..r.right() as usize];
        layout.convert_row(src, &mut row[..src.len()]);      // → row を framebuffer に書く (統合側)
    }
}
```

## 4. API 一覧

### 4.1 色・矩形・ダメージ

| 型 / 関数 | 内容 |
| --- | --- |
| `Color`, `blend_over`, `palette::*` | §1。パレットはカーネルの `graphics::color::palette` と同じ値 |
| `Rect` | `new`, `from_size`, `from_ltrb`, `is_empty`, `left/top/right/bottom`, `area`, `contains_point`, `contains`, `intersects`, `intersect`, `clamp_to`, `union`, `translate`, `inflate` |
| `DamageList` | `add` (空は無視、含まれる矩形は追加しない、新しい矩形に含まれる既存は削除、重なる/接する矩形は **外接矩形にしても無駄な面積が 25% 以内ならマージ**、数が `max_rects` を超えたら全体を 1 つの外接矩形に)、`take`, `rects`, `bounds`, `clip_to`, `clear` |

### 4.2 Image / Canvas

| 型 / 関数 | 内容 |
| --- | --- |
| `Image` (= `Surface`) | 所有バッファ (`Vec<u32>`、stride == 幅)。`new`, `filled`, `from_pixels`, `canvas()`, `view()`, `get`, `crop`, `scaled`, `multiply_alpha`, `pixels(_mut)`, `into_pixels` |
| `ImageView` | 借用の読み取り専用ビュー (`data, width, height, stride`)。`new` (検証付き)、`get`, `row`, `sub_view`, `to_image` |
| `Canvas::new(buf, w, h, stride)` | 借用バッファの描画面 (検証付き)。`clip`/`set_clip`/`reset_clip`、`sub_canvas`、`reborrow`、`as_view` |
| ピクセル | `get_pixel`, `set_pixel` (上書き), `blend_pixel` (合成), `clear`, `clear_rect` |
| 矩形 | `fill_rect`, `stroke_rect(rect, thickness, color)` (内側に太さ `thickness`)、`hline`, `vline` |
| 線 | `line(x0, y0, x1, y1, thickness, color)`、`line_aa` (§5.1) |
| 角丸矩形 | `fill_round_rect(rect, radius, color)`、`stroke_round_rect`、`fill_round_rect_gradient_v/h` (§5.2) |
| 円・楕円 | `fill_ellipse(rect)`, `stroke_ellipse(rect, thickness)`, `fill_circle(cx, cy, r)`, `stroke_circle` (§5.3) |
| グラデーション | `fill_gradient_v(rect, top, bottom)`, `fill_gradient_h(rect, left, right)` |
| マスク | `draw_mask(x, y, &MaskRef, color)` (8bit カバレッジ。グリフ描画の下請け) |
| 画像転送 | `blit` (不透明コピー)、`blit_region`、`blit_alpha`、`blit_opacity(…, opacity)` (全体アルファ乗算)、`blit_ex(src, src_rect, dx, dy, BlendMode, opacity)`、`copy_within(src_rect, dx, dy)` (重なりOK・スクロール用)、`blit_scaled(src, src_rect, dst_rect, Filter, BlendMode, opacity)` |

- **Surface 間の矩形コピー**: `dst.blit_region(&src_canvas.as_view(), rect, dx, dy)`。行ごとに `copy_from_slice` (memcpy) で転送する。
- `BlendMode::Copy` はアルファも含めて **上書き** (透明ピクセルも書き込まれるので、不透明なバックバッファに透明画像を
  `blit` すると黒い穴になる)。重ねるときは `blit_alpha` / `BlendMode::SourceOver`。
- `Filter::Nearest` / `Filter::Bilinear` (アルファ重み付きの 4 点補間。透明と不透明の補間で色が暗くならない)。
  ピクセル中心どうしを対応づける (`src = (dst + 0.5) * sw/dw - 0.5`)。**縮小率が 1/2 より小さいとバイリニアでもエイリアシング
  が出る** (ミップマップ/ボックスフィルタは未実装)。
- 拡縮の座標テーブルは `Vec` (クリップ後の幅ぶん) を 1 つ確保する。`fill_gradient_h` も 1 行ぶんの `Vec` を確保する。

### 4.3 PNG (`yk_gfx::png`)

`decode_png(&[u8], &PngLimits) -> Result<Image, PngError>`、`png_info(&[u8]) -> Result<PngInfo, PngError>` (IHDR のみ)。

| 項目 | 対応 |
| --- | --- |
| カラータイプ | グレースケール / グレー+アルファ / RGB / RGBA / パレット (PLTE + tRNS、パレット数が 2^depth より少なくてもよい) |
| ビット深度 | 1, 2, 4, 8, 16 (16bit は上位 8bit に落とす) |
| インターレース | なし / **Adam7** |
| APNG | 既定画像 (IDAT) のみ。アニメーションは無視 |
| 色空間・ガンマ・ICC・pHYs | 無視 (sRGB 扱い) |

安全性 (すべてテスト済み):

- **展開前に IHDR を自前で検証** し、`PngLimits { max_width, max_height, max_pixels, verify_crc }` を超える画像は
  **何も確保せず** `PngError::TooLarge` で拒否する。既定は 8192x8192・1600 万画素・CRC 検証あり。
  限界を `u64::MAX` にしても `Image::MAX_PIXELS` (2^28) で拒否される。
- 小さな IHDR に対して巨大に展開される IDAT (展開爆弾、`tests/data/bomb_idat.png` = 39KB で 40MB に展開) は、
  期待サイズを超えた時点で `Err` (拒否に 1ms 未満、ピーク RSS 13MB)。
- 壊れた入力: 8 ファイル x 60,000 回の乱数変異 (バイト書き換え・切り詰め・IHDR 破壊・圧縮データ破壊、CRC 検証あり/なし)
  = 約 96 万回のデコードで **パニックなし** (debug ビルド = オーバーフロー検査あり)。`YK_FUZZ_ITERS=60000 cargo test
  --test png fuzz` で再現できる (既定のテストは 1500 回/ファイル)。
- **メモリ**: デコード中のピーク ≈ 出力 `Image` (4B/画素) + 展開後の生データ (1〜4B/画素) + 展開バッファ
  ≈ **出力サイズの約 3 倍** (1920x1080 RGBA で 23.8MiB、RGB で 19.8MiB を実測)。カーネルヒープが小さい場合は
  `PngLimits::max_pixels` を絞ること。

### 4.4 テキスト (`yk_gfx::text`)

| 型 / 関数 | 内容 |
| --- | --- |
| `Font::new(&'a [u8])` | TrueType/OpenType を解析 (遅延解析・ヒープ確保なし)。`has_glyph(char)`, `metrics(size) -> FontMetrics {ascent, descent, line_gap, line_height}` |
| `Font::measure_text(size, text) -> TextMetrics {width, height, lines}` | 改行を含む文字列の寸法 (折り返しなし)。`measure_wrapped(size, text, max_width)` |
| `Font::layout(size, text, max_width: Option<u32>, line_height: Option<f32>) -> TextLayout` | 行分割 (`lines: Vec<TextLine {start, end, width64}>`、`line_height`, `baseline`, `box_width`) |
| `TextStyle { size, color, align, line_height }` | `TextStyle::new(size, color).align(TextAlign::Center).line_height(px)` |
| `TextRenderer::new()` / `with_cache_capacity(bytes)` | グリフキャッシュを持つ描画器 (フォントは呼び出しごとに渡すので、Regular/Bold で 1 つのキャッシュを共有できる) |
| `TextRenderer::draw_text(canvas, font, style, x, y, text) -> u32` | 1 行描画。`(x, y)` は **行ボックスの左上**。描いた幅 (px) を返す。改行以降は描かない |
| `TextRenderer::draw_text_box(canvas, font, style, rect, text) -> u32` | `rect` の幅で折り返して描く。はみ出しは `rect` でクリップ。`style.align` は `rect` の幅に対する左/中央/右。描いた行数を返す |
| `cache_stats()`, `clear_cache()`, `set_cache_capacity()` | `CacheStats {entries, bytes, max_bytes, hits, misses, evicted}` |

仕様:

- **サイズ**は em の大きさ (px、CSS の `font-size` と同じ)。`f32` で渡す (13.33 など可)。非有限値・範囲外は
  `[1, 1024]` に丸める。キャッシュのキーは 1/8px 単位に量子化。
- **配置は整数 (26.6 固定小数点)**: 送り幅 = `hmtx` の値 x サイズ / unitsPerEm。グリフは整数ピクセル位置に置く
  (サブピクセル位置決めなし)。カーニング (GPOS/kern)・合字・結合文字の位置合わせ・BiDi・縦書きは **未対応**。
- **行の寸法**: 標準の行送り = `ascent + descent + line_gap`。`line_height` を指定すると標準の高さ (`ascent+descent`) を
  行送りの中央に置く (half-leading)。`TextLayout::baseline` は最初の行の上端からベースラインまで。
- **折り返し**: `\n` は常に改行、`\r` は無視。`max_width` があると、空白の後・CJK 文字の前後・ハイフンの後で改行する。
  英単語の途中では切らない (ただし 1 単語が幅より長いときは文字単位で割る)。行末の空白は捨てる。
  **禁則処理 (主要な文字のみ)**: 行頭に `、。，．・：；？！）］｝〕〉》」』】ー々`… と小書き仮名を置かない
  (はみ出してもその行に置く)、行末に開き括弧を置かない。ぶら下げ・約物の詰めは未対応。
- **未収録の文字は豆腐 (□)**: サイズに比例した枠 (幅 0.6em・高さ 0.7em・送り 0.8em)。制御文字・ゼロ幅文字
  (U+200B–200F, U+FE00–FE0F, U+FEFF など) は幅 0 で描かない。タブは空白 4 つぶん。
- **グリフキャッシュ**: キー = (フォント, グリフ ID, サイズ)。値 = 8bit カバレッジビットマップ + 配置オフセット。
  容量 (既定 2 MiB、概算 = ビットマップ + エントリあたり 96B) を超えたら **最終使用が古い順に容量の半分まで破棄**。
  容量の 1/4 を超える巨大グリフはキャッシュせず毎回ラスタライズ。容量 0 ならキャッシュなし。
  `BTreeMap` を使うので決定的 (ハッシュ不要)。
- **ラスタライズ** (キャッシュミス時のみ): `ab_glyph` がアウトラインを 8bit カバレッジにする (ここだけ浮動小数点)。
  描画は `Canvas::draw_mask` (整数演算) で、色のアルファにカバレッジを掛けて source-over。ガンマ補正はしない (線形ブレンド)。

### 4.5 ヒープ不要のビットマップフォント (`yk_gfx::bitmap`)

早期コンソール/パニック画面向け。フォントデータは呼び出し側が渡す (`font8x8` の `[u8; 8]`: 各行 1 バイト、LSB が左)。

- `draw_glyph8(canvas, x, y, &[u8; 8], scale, color)`: **任意の整数倍スケール**。連続するビットを 1 つの矩形にまとめて塗る。
- `draw_text8(canvas, x, y, text, scale, color, lookup: Fn(char) -> Option<&[u8; 8]>) -> (i64, i64)`:
  `lookup` が `None` の文字は四角の豆腐。`\n` で次の行。
- `measure_text8(text, scale)`。

`Canvas` を作るだけでヒープは使わない (`Canvas::new(&mut [u32], ..)` で既存のバッファに描ける)。
framebuffer を直接 `&mut [u32]` として借りられない構成のときは、`Image` のバックバッファに描いてから転送する。

### 4.6 framebuffer 形式への変換 (`yk_gfx::present`)

`PixelLayout { red, green, blue: Channel { shift, size } }`。Limine/UEFI が申告する R/G/B のマスクから作る。
`encode(argb)` (アルファは無視、各チャネルを `size` ビットに丸めて `shift` に置く。`size` は 1〜16)、
`convert_row(src, dst)` (`XRGB8888` = R16/G8/B0 のときは `& 0x00FFFFFF` だけの高速経路)。

## 5. 図形の仕様

### 5.1 線

- `line(.., thickness, color)`:
  - `thickness == 0` → 何もしない。`1` → **Bresenham (アンチエイリアスなし)**、両端のピクセルを含む。
  - 軸に平行で `thickness >= 2` → 中心線を挟んだ矩形 (角ばった端点、整数ピクセル。厚みが偶数のときは下/右側が 1px 厚い)。
  - 斜めで `thickness >= 2` → `line_aa` (丸い端点、アンチエイリアス)。
- `line_aa`: 端点はピクセル中心。1/16 ピクセル精度の整数距離 (`i128`) で「線分までの距離 ≤ 半幅 + 0.5px」の傾斜をカバレッジにする。
  **時間は線の外接矩形 (クリップ後) の面積に比例** (1000px の斜め 4px 線で約 3ms)。短い線 (UI の装飾) 向け。

### 5.2 角丸矩形

- `radius` は `min(w, h) / 2` (と 2^24) に制限される。角は **1/16 ピクセル精度の円との距離** でアンチエイリアス。
  直線部分は `fill_rect` と同じ行単位の塗り (高速)。`radius == 0` は矩形と完全に一致。
- `stroke_round_rect`: 外側の角丸矩形から、太さ分だけ内側に縮めた角丸矩形 (半径 `r - thickness`) を引いたカバレッジ。
  空洞の中央部分は走査しない。半透明色でも重なりで二重に濃くならない。
- `fill_round_rect_gradient_v/h`: 同じ形をグラデーションで塗る (色は矩形全体に対して決まる)。
  **`set_clip` と組み合わせると「上だけ角丸のタイトルバー」** が作れる (`examples/showcase.rs` の `window()`)。
- 4 回対称であることをテストで確認している。

### 5.3 円・楕円

矩形に内接する楕円。各行を **縦 4 サブサンプル x 横 1/16 ピクセル精度** の面積カバレッジでアンチエイリアス
(1 サブ行ごとに `isqrt` で厳密な x 区間を求め、ピクセルとの重なりを積分)。内部は一括して塗れる範囲を行単位で塗り、
枠だけが per-pixel。円の面積が理論値 πr² と 2% 以内、上下左右対称であることをテストで確認している。
`stroke_ellipse` は内側の楕円 (半径 − thickness) を引く。厚みが半径以上なら塗りつぶし。

### 5.4 グラデーション

`fill_gradient_v/h` は 8bit/チャネルの線形補間 (端点の色が両端の画素にぴったり出る)。ディザリングはしない
(淡い色の広い範囲では 8bit の段差が見えることがある)。色はクリップ前の矩形に対して決まるので、
クリップしても色がずれない (テストで確認)。

## 6. フォント

| 項目 | 内容 |
| --- | --- |
| 採用 | **M PLUS Rounded 1c** (丸ゴシック、日本語含む、SIL OFL 1.1) |
| 入手元 | <https://github.com/google/fonts/tree/ce6523df69fcaec9534b128945bf47cd92bdc5b2/ofl/mplusrounded1c> (google/fonts のコミット `ce6523df69fcaec9534b128945bf47cd92bdc5b2`、`git clone --filter=blob:none --sparse` で取得) |
| ファイル | `assets/fonts/MPLUSRounded1c-Regular.ttf` (3,389,564 B)、`MPLUSRounded1c-Bold.ttf` (3,542,384 B)、`OFL.txt`、`SHA256SUMS`、`SOURCE.txt` |
| sha256 | Regular `fd003ca21668b7f1ab5665accd1a785587b75e18b8edff65c663fa6facefd882` / Bold `c698330e24aadd0da80cab1f321eee9c052d41b66ef84e7e5379e81fb084a890` (`cd assets/fonts && sha256sum -c SHA256SUMS`) |
| 収録 | 8,546 グリフ (cmap 8,201 文字)。ひらがな・カタカナ全部、**常用漢字 2,136 字すべて**、JIS X 0208 第 1 水準 2,965 字すべて、第 2 水準は 3,390 字中 1,943 字 (1,447 字は未収録 → 豆腐)。ASCII。絵文字なし |
| 容量 | Regular のみなら 3.4MB、Bold も入れると **+3.5MB (計 6.9MB)** |

- OFL.txt: google/fonts のこのファミリのディレクトリには OFL.txt が無いため、同じコミットの `ofl/mplus1p/OFL.txt` (同じ M+ プロジェクトの
  標準の OFL 1.1 全文) の著作権表示行だけを、フォントの name テーブル (ID 0) と METADATA.pb にある
  `Copyright 2016 The Rounded M+ Project Authors.` に差し替えて置いた。**Reserved Font Name の指定はない**。
- 代替候補 (Zen Maru Gothic、Noto Sans JP) は **使っていない** (第一候補を取得でき、常用漢字を完全に収録しているため)。
  `ofl/zenmarugothic` は同じ sparse clone で取得可能なことを確認している (Regular/Bold/Black/Light/Medium)。
- クレートはフォントを内蔵しない。呼び出し側が `&'static [u8]` を渡す (`include_bytes!` またはファイルシステムから読んだバッファ)。
  フォントデータは **解析でコピーされず、`Font` が借用するだけ** なので、3.4MB がそのままヒープに複製されることはない。

## 7. ダメージ管理

`DamageList` はコンポジタが「再描画が必要な領域」を集める小さな型 (§4.1)。ウィンドウ移動や再描画のたびに `add` し、
フレームの最後に `take()` した矩形だけをバックバッファ → framebuffer に転送する。上限 (`max_rects`、既定 16) を超えると
1 つの外接矩形になるので、矩形数は有界。`clip_to(screen)` で画面外を落とす。

## 8. 検証

### 8.1 コマンド

```sh
# ホストでのテスト (6 ファイル・88 件)
cargo test -p yk-gfx --target x86_64-unknown-linux-gnu
# カーネルにリンクできるターゲット (soft-float)
cargo build -p yk-gfx --target x86_64-unknown-none            # debug
cargo build -p yk-gfx --target x86_64-unknown-none --release
# 静的検査
cargo clippy -p yk-gfx --target x86_64-unknown-none
cargo clippy -p yk-gfx --target x86_64-unknown-linux-gnu --all-targets
cargo fmt --check
# 既存カーネルを壊していないこと
cargo build --locked -p yumekawa-kernel --target x86_64-unknown-none
# 目視確認 (build/gfx/showcase.png, 1280x800)
cargo run -p yk-gfx --example showcase --target x86_64-unknown-linux-gnu
# ベンチ
cargo run --release -p yk-gfx --example bench --target x86_64-unknown-linux-gnu
```

`.cargo/config.toml` の既定ターゲットは `x86_64-unknown-none` なので、**ホストで動かすときは必ず `--target x86_64-unknown-linux-gnu`**
(`-p yk-gfx` なしのワークスペース全体のビルドはホストでは失敗する: カーネルが none 専用のため)。

### 8.2 テストの内容

| ファイル | 内容 |
| --- | --- |
| `tests/rect.rs` | Rect の演算と境界 (半開区間、極端な値でのオーバーフロー)、DamageList (含有・マージ・上限・乱数での取りこぼしなし) |
| `tests/canvas.rs` | 色/アルファ合成の数値、図形のピクセル単位の期待値 (ASCII 表示で比較)、対称性・面積、クリッピング (負座標・画面外・巨大サイズ・`i32::MIN/MAX`)、グラデーション、画像転送・拡縮・`copy_within`、ガードバンドの乱数ファジング (全 API) |
| `tests/png.rs` | 全カラータイプ/ビット深度/Adam7 の正常系 (期待値を式から計算)、tRNS、サイズ上限、巨大な宣言サイズ、展開爆弾、切り詰め/不正ヘッダ、CRC、乱数変異ファジング |
| `tests/text.rs` | 常用漢字・かな・JIS 第 1 水準の収録確認、全かなが描かれること、メトリクスと幅計測、ゼロ幅/制御文字、豆腐、キャッシュ (ヒット・容量上限・破棄・巨大グリフ)、極端なサイズ、折り返し (英語/日本語/禁則/改行)、矩形内の描画とクリップ・揃え、ボールド、ガードバンドの乱数テスト |
| `tests/bitmap.rs`, `tests/present.rs` | ビットマップフォント、画素形式変換 |

### 8.3 soft-float でのリンク・実行確認 (`gui/gfx/tools/softfloat-probe/`)

`x86_64-unknown-none` (SSE/x87 なし) の freestanding ELF として yk-gfx を使うプログラムをビルドし、**Linux 上でそのまま実行** して確認した
(syscall だけを直接叩く使い捨てコード。ワークスペース外・`unsafe` あり・カーネルには入らない):

- 未解決シンボルなしでリンクできる (`compiler_builtins` の `__addsf3` 等のみ使用)。
- `objdump -d` で `xmm`/`ymm`/x87 命令が **0 個**。
- PNG・テキスト・図形が soft-float でも正しく動く (PNG のピクセル値がホスト版と一致)。
- 速度 (§10 の表の「soft-float」列)。

```sh
cd gui/gfx/tools/softfloat-probe
RUSTFLAGS="-C relocation-model=static" cargo build --release --target x86_64-unknown-none
./target/x86_64-unknown-none/release/softfloat-probe
```

## 9. カーネル統合担当への引き継ぎ

### 9.1 初期化手順 (推奨)

1. カーネル (または GUI サーバ) の `Cargo.toml` に `yk-gfx = { path = "../gui/gfx" }` を追加する。
   **`unsafe` を使わないクレートなので、カーネル側の `// SAFETY:` 規約の対象外**。`x86_64-unknown-none` で `alloc` が使えれば動く
   (ヒープ初期化後に使うこと)。
2. バックバッファを確保: `let mut back = Image::new(fb.width(), fb.height())?;` (1280x800 = 4,096,000 B)。
   確保失敗は `Err(GfxError::OutOfMemory)` で返る (abort しない)。
3. フォントは `static REGULAR: &[u8] = include_bytes!("…/assets/fonts/MPLUSRounded1c-Regular.ttf");`
   (カーネルイメージが 3.4MB 増える。ファイルシステムが入った後は読み込んだバッファを `Box::leak` して `&'static [u8]` にしてもよい)。
   `Font::new(REGULAR)` は 13〜25µs、ヒープ確保なし。`TextRenderer::new()` を 1 つ持つ (`static` にするなら `IrqSpinLock` などで保護。
   `TextRenderer`/`Font` は内部に `unsafe` やスレッドローカルを持たず、`&mut` を要求するだけ)。
4. フレームの描画: `back.canvas()` に描く → `DamageList` の矩形だけを framebuffer に転送。

### 9.2 バックバッファ → framebuffer の転送 (**統合側の責務**)

- バックバッファは **ARGB8888 (alpha は使わない場合 0xFF が望ましいが、変換で捨てる)**。framebuffer の形式は Limine が申告する
  R/G/B の `shift`/`size`。`PixelLayout { red: Channel { shift, size }, .. }` を framebuffer の `PixelFormat` から作り、
  `convert_row(src_row, tmp_row)` で 1 行ずつ変換してから framebuffer に書く。一般的な UEFI GOP (`B=0, G=8, R=16`) では
  `PixelLayout::XRGB8888` になり、変換は `& 0x00FFFFFF` だけ (メモリ書き込みが律速)。
- framebuffer は WC マップで **読み出しが遅い**: バックバッファ → framebuffer の一方向だけにすること。`volatile` 書き込みは
  カーネル側 (`graphics::framebuffer`) で行う。yk-gfx は framebuffer を知らない。
- `graphics::display::with` のロック中は割り込み禁止なので、全画面転送は **矩形/行単位に分割** する
  (`docs/architecture.md` §9)。`DamageList::take()` の矩形を 1 つずつ、または数十行ずつ処理する。
- 透明ピクセル (`a = 0`) を `Copy` でバックバッファに書くと黒になる。重ねる画像は `blit_alpha`。
  バックバッファの最背面 (壁紙) を不透明で塗れば、以後の `a < 255` の描画はすべて不透明のまま (高速経路) になる。

### 9.2.1 早期コンソール/パニック画面

`graphics::font` (font8x8) の置き換え先として `yk_gfx::bitmap::draw_text8` が使える (ヒープ不要)。ただし `Canvas` は
`&mut [u32]` を要求するので、framebuffer を直接 `Canvas` にするには framebuffer メモリを `&mut [u32]` として借りる必要がある
(→ カーネル側で `unsafe` が要る)。パニック画面は現状の `Framebuffer` 直接描画のままにするのが無難。

### 9.3 ヒープ使用量の目安

| 項目 | 量 |
| --- | --- |
| バックバッファ 1280x800 | 4.0 MB (3.9 MiB) |
| 追加のレイヤー/ウィンドウバッファ | `幅 x 高さ x 4` バイト |
| グリフキャッシュ | 既定 **上限 2 MiB** (`with_cache_capacity` で変更)。日本語 (14〜24px) の一般的な画面で数百グリフ・約 100KB (showcase: 269 エントリ 88KiB) |
| `Font` | ヒープなし (フォントデータは借用) |
| グリフのラスタライズ時 | 一時的に `幅 x 高さ x 4` バイト程度 (ab_glyph の累積バッファ。ミスのたびに確保・解放) |
| PNG デコード | **出力の約 3 倍** (1920x1080 RGBA で 24 MiB)。`PngLimits` で制限する |
| 拡縮 (`blit_scaled`) | クリップ後の幅 x 12 バイト |
| `fill_gradient_h` | クリップ後の幅 x 4 バイト |
| `layout` / `draw_text_box` | 文字数 x 24 バイト程度 (一時) |

### 9.4 注意点

- **割り込みハンドラ内で使わない** (ヒープを確保する)。
- **パニックの可能性**: ライブラリのコードは範囲外アクセスを起こさないよう検証しているが、`panic = "abort"` のカーネルでは
  内部バグ (添字範囲外など) があればパニックになる。極端な引数でのパニックは無いことをファジングで確認している。
- **ホストと実機の差**: soft-float ではグリフのラスタライズ (キャッシュミス) が 1 文字 約 45〜100µs (ホストの SSE 版は 5〜10µs)。
  初回表示や大量の新しい漢字を一度に出すとき (1,700 字で約 160ms) だけ目立つ。ウィンドウ内の文字を事前に
  描いてキャッシュを温めておく、1 フレームに描く新規グリフ数を制限する、といった対策が有効。
- **スレッド安全性**: `Canvas`/`TextRenderer` は `&mut` で使う (内部可変性なし)。マルチコアで共有するなら呼び出し側でロックする。
  `Font` の ID 割り当てだけ `AtomicU32` を使う。
- **スタック使用量**: 最大の関数でも数 KB 程度 (固定長配列は `[u8; 320]` 級まで)。再帰なし (`ab_glyph`/`ttf-parser` の
  複合グリフの再帰はフォント側の深さ制限に従う)。
- **バージョン固定**: 依存は `=x.y.z` で固定し `Cargo.lock` をコミット済み (`docs/dependencies.md` §4)。
- 既存の `graphics::color::Rgb` と `yk_gfx::Color` は別の型 (変換は `Color::rgb(r, g, b)`)。

## 10. 性能 (参考値)

1280x800 (1,024,000 画素)。ホストは `cargo run --release -p yk-gfx --example bench` (SSE が使えるコンパイル)、soft-float は
§8.3 の `softfloat-probe` (SSE/x87 を使わないバイナリを同じホスト CPU で実行)。**実機/QEMU での計測はしていない**。

| 処理 | ホスト (best) | soft-float (best) |
| --- | --- | --- |
| 全面 `fill_rect` 不透明 | 0.13 ms | 0.17 ms |
| 全面 `fill_rect` 半透明 (a=128) | 1.7 ms | 1.7 ms |
| 全面 縦グラデーション | 0.18 ms | 0.17 ms |
| 全面 横グラデーション | 0.18 ms | — |
| 全面 `blit` (不透明コピー) | 0.29 ms | — |
| 全面 `blit_alpha` (ソースが全部不透明) | 0.94 ms | — |
| 全面 `blit_alpha` (透明・半透明が混在) | 2.2 ms | 3.2 ms (一様な半透明) |
| 全面 `blit_opacity` 50% | 4.5 ms | — |
| 全面 `copy_within` (16 行スクロール) | 0.13 ms | — |
| `blit_scaled` 最近傍 640x400→1280x800 | 0.48 ms | — |
| `blit_scaled` バイリニア 640x400→1280x800 | 5.5 ms | 5.4 ms |
| `fill_round_rect` 600x400 | 0.03 ms | 0.06 ms |
| `fill_ellipse` 600x400 | 0.07 ms | 0.08 ms |
| `line_aa` 太さ 4・斜め 1000px | 3.2 ms | — |
| 日本語 148 文字 14px 折り返し描画 (キャッシュあり、ヒット) | 0.07 ms (0.19 ms は全面クリア込み) | 0.06 ms |
| 同 24px | 0.14 ms | 0.12 ms |
| 同 14px (キャッシュなし = 毎回ラスタライズ) | 0.9 ms (全面クリア込み、6 µs/文字) | — |
| 同 14px (キャッシュ空、初回 78 グリフ) | 0.6 ms | 3.3 ms (42 µs/グリフ) |
| 同 24px (キャッシュ空、初回 78 グリフ) | 0.8 ms | 4.2 ms (54 µs/グリフ) |
| 漢字 1,728 字 20px (全部違うグリフ、キャッシュ空) | 約 11 ms/1,000 字 | 164 ms (94 µs/グリフ) |
| レイアウトのみ (148 文字) | 0.005 ms | — |
| PNG デコード 512x512 RGBA (354KB) | 3.1 ms | — |
| PNG デコード 1024x768 RGB (8KB、よく圧縮される) | 4.3 ms | — |
| `Font::new` (3.4MB) | — | 13〜25 µs |

- 全面の塗り・コピー・合成の内側ループは **整数演算のみ** なので soft-float の影響を受けない (同程度)。
  浮動小数点を使うのは `ab_glyph` のラスタライズ (キャッシュミス時) と `Font::metrics` 等の `f32` サイズの換算のみ。
- 半透明の全面塗り/合成が不透明の 10 倍以上遅いので、**コンポジタは不透明ウィンドウを `blit` (Copy) し、半透明は必要な矩形だけに**
  するとよい。DamageList で更新領域を絞るのが最大の最適化。

## 11. 未達・既知の制限

- **実機/QEMU での性能・動作の計測は未実施** (framebuffer への統合は後続フェーズ)。
- フォント: JIS X 0208 第 2 水準の一部 (1,447 字) と絵文字は M PLUS Rounded 1c に収録されておらず豆腐になる
  (フォールバックフォントの連鎖は未実装。必要なら `TextRenderer` に複数 `Font` を渡す拡張が要る)。
- テキスト: カーニング・合字・結合文字・BiDi・縦書き・サブピクセル位置決め・下線/取り消し線・ぶら下げ・
  「約物の詰め」は未対応。禁則は主要な文字のみ。
- 画像: バイリニアは大きな縮小でエイリアシングが出る (ミップマップ/ボックスフィルタ未実装)。回転・アフィン変換・反転なし。
  JPEG/GIF/WebP のデコードなし (ブラウザ用に将来追加)。
- 図形: 多角形・ベジェ・パスの塗りなし。太線は丸い端点のみ (butt/square cap なし)、折れ線の結合 (join) なし。
  `line_aa` は外接矩形の面積に比例して遅い (長い太線には不向き)。
- ブレンド: 非プリマルチプライドなので、半透明どうしを重ねるとチャネルごとの除算が入り遅い (不透明な背景に重ねるのが前提)。
- ガンマ補正・色管理なし (sRGB 値をそのまま線形に混ぜる)。
- PNG: ガンマ/ICC/pHYs/アニメーションは無視。16bit は上位 8bit に落とす。
- `Font` は TrueType コレクション (`.ttc`) と可変フォントの軸指定に未対応。
