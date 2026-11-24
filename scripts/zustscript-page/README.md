# zustscript 入門記事の編集

原稿は [crates/zustscript/docs](../../crates/zustscript/docs) にあります。公開する記事とサイドバーの表示名は [render.mjs](render.mjs) で指定します。最初の H1 はページタイトル、最初の段落はページの説明文にも使います。コードフェンスには `zustscript`、`typescript`、`javascript`、`bash`、`rust` のいずれかを指定すると Tree-sitter が色を付けます。実行結果には `text` を使います。

リポジトリのルートで次を実行すると、各記事の HTML が `out/pages/zustscript/` にできます。Node.js 24、pnpm 10、just、Tree-sitter CLI 0.26.8 と C コンパイラを使います。プレビューには公開サイトの共通スタイルを添えます。

```sh
just -f scripts/zustscript-page/justfile build
git show origin/gh-pages:style.css > out/pages/style.css
python3 -m http.server 8000 --directory out/pages
```

ブラウザで [ローカルの紹介ページ](http://localhost:8000/zustscript/) を開きます。レイアウトの変更箇所は [template.html](template.html) です。

PR では生成した HTML を Actions の artifact から確認できます。原稿、テンプレート、生成ツール、zustscript の文法の変更が main に入ると、[専用の workflow](../../.github/workflows/zustscript-page.yml) が記事を生成し、GitHub Pages の [zustscript 入門](https://mizchi.github.io/zust/zustscript/) を更新します。
