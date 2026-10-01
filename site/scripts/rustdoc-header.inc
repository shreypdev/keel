<!-- Undra's mark on rustdoc (passed as --html-in-header by site/scripts/build-rustdoc.sh): the site's icon, Geist, the orange accent
     and the site's two palettes, set as rustdoc's own theme variables. rustdoc's layout is untouched. -->
<link id="undra-icon" rel="icon" type="image/svg+xml" href="/undra/favicon.svg">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Geist:wght@400..600&amp;family=Geist+Mono:wght@400..500&amp;display=swap">
<style>
:root { --font-family: "Geist", system-ui, sans-serif; --font-family-code: "Geist Mono", ui-monospace, monospace; }
.sidebar-crate::before { content: ""; display: block; width: 28px; height: 28px; margin-bottom: 10px; background: url(/undra/favicon.svg) center / contain no-repeat; }
#crate-search, h1, h2, h3, h4, h5, h6, .sidebar, rustdoc-topbar, .search-input, .search-results .result-name, .item-table dt > a, .out-of-band, .sub-heading, span.since, a.src, rustdoc-toolbar, summary.hideme, .scraped-example-list, .rustdoc-breadcrumbs, .search-switcher, ul.all-items, .deprecated-count { font-family: "Geist", system-ui, sans-serif; }
:root[data-theme="ayu"], :root[data-theme="dark"] {
  --main-background-color: #0a0a0a; --main-color: #c9c9c4; --sidebar-background-color: #0e0e0e; --sidebar-background-color-hover: #1d1d1d;
  --sidebar-border-color: #232323; --border-color: #353535; --headings-border-bottom-color: #232323; --code-block-background-color: #0d0d0d;
  --button-background-color: #111; --link-color: #ff9a6b; --type-link-color: #ff9a6b; --assoc-item-link-color: #ff9a6b; --mod-link-color: #ff9a6b;
  --keyword-link-color: #ff9a6b; --attribute-link-color: #ff9a6b; --trait-link-color: #c6aaf0; --macro-link-color: #c4d99a; --function-link-color: #ededea;
  --sidebar-link-color: #ff9a6b; --sidebar-current-link-background-color: rgba(255, 106, 42, .11); --target-border-color: #ff6a2a;
  --target-background-color: rgba(255, 106, 42, .11); --search-input-focused-border-color: #ff6a2a; --settings-input-color: #ff6a2a;
  --search-tab-button-selected-border-top-color: #ff6a2a; --code-highlight-kw-color: #ff9a6b; --code-highlight-kw-2-color: #ff9a6b;
  --code-highlight-string-color: #c4d99a; --code-highlight-number-color: #e6c07b; --code-highlight-comment-color: #86867f; --code-highlight-doc-comment-color: #86867f;
}
:root[data-theme="light"] {
  --main-background-color: #f7f6f2; --main-color: #2c2c29; --sidebar-background-color: #f1f0eb; --sidebar-background-color-hover: #efeee9;
  --sidebar-border-color: #e2e0d9; --border-color: #cbc8bf; --headings-border-bottom-color: #e2e0d9; --code-block-background-color: #fbfaf7;
  --button-background-color: #fff; --link-color: #c2410c; --type-link-color: #b23a09; --assoc-item-link-color: #b23a09; --mod-link-color: #b23a09;
  --keyword-link-color: #b23a09; --attribute-link-color: #b23a09; --trait-link-color: #6d28d9; --macro-link-color: #4d7c0f; --function-link-color: #141413;
  --sidebar-link-color: #c2410c; --sidebar-current-link-background-color: rgba(194, 65, 12, .08); --target-border-color: #c2410c;
  --target-background-color: rgba(194, 65, 12, .08); --search-input-focused-border-color: #c2410c; --settings-input-color: #c2410c;
  --search-tab-button-selected-border-top-color: #c2410c; --code-highlight-kw-color: #b23a09; --code-highlight-kw-2-color: #b23a09;
  --code-highlight-string-color: #4d7c0f; --code-highlight-number-color: #92590a; --code-highlight-comment-color: #6f6e66; --code-highlight-doc-comment-color: #6f6e66;
}
</style>
<script>(function () { // our icon replaces rustdoc's; follow the site's theme choice (same origin: the key is shared) or ?theme=light|dark
  document.querySelectorAll('link[rel$="icon"]:not(#undra-icon)').forEach(function (l) { l.remove(); });
  try {
    var m = /[?&]theme=(light|dark)/.exec(location.search), t = m ? m[1] : localStorage.getItem('undra-theme');
    if (t === 'light' || t === 'dark') document.documentElement.setAttribute('data-theme', t === 'light' ? 'light' : 'ayu');
  } catch (e) { /* storage blocked: rustdoc's own choice stands */ }
})()</script>
