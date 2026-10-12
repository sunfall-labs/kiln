#!/usr/bin/env python3
"""Write canonical URLs, social metadata, breadcrumbs and an XML sitemap."""
import html
import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE = 'https://sunfall-labs.github.io/kiln/'
paths = [ROOT/'index.html', *sorted((ROOT/'reference').glob('*.html')), *sorted((ROOT/'compare').glob('*.html'))]
ns = 'http://www.sitemaps.org/schemas/sitemap/0.9'
ET.register_namespace('',ns)
sitemap = ET.Element(f'{{{ns}}}urlset')
for path in paths:
    markup = path.read_text()
    markup = re.sub(r'\n?<!-- generated:seo -->.*?<!-- /generated:seo -->\n?', '', markup, flags=re.S)
    # Replace the original homepage social fields as well as any generated fields.
    markup = re.sub(r'\s*<meta\b[^>]*(?:property="og:[^"]*"|name="twitter:[^"]*")[^>]*>', '', markup)
    markup = re.sub(r'\s*<link\b[^>]*rel="canonical"[^>]*>', '', markup)
    title = html.unescape(re.search(r'<title>(.*?)</title>',markup,re.S).group(1))
    description = html.unescape(re.search(r'<meta name="description" content="([^"]*)"',markup).group(1))
    relative = path.relative_to(ROOT).as_posix()
    canonical = BASE + (relative[:-10] if relative.endswith('index.html') else relative)
    schema = {'@context':'https://schema.org','@type':'WebPage','name':title,'description':description,'url':canonical,'inLanguage':'en'}
    if relative != 'index.html':
        parent = relative.split('/')[0]
        crumbs = [('kiln',BASE),('Comparisons' if parent == 'compare' else 'Reference',BASE+parent+'/')]
        if path.name != 'index.html':crumbs.append((title.removesuffix(' — kiln').removesuffix(' — kiln reference'),canonical))
        schema['breadcrumb']={'@type':'BreadcrumbList','itemListElement':[{'@type':'ListItem','position':i,'name':label,'item':url} for i,(label,url) in enumerate(crumbs,1)]}
    esc = lambda value:html.escape(value,quote=True)
    tags = f'''\n<!-- generated:seo -->
<link rel="canonical" href="{canonical}">
<meta property="og:title" content="{esc(title)}">
<meta property="og:description" content="{esc(description)}">
<meta property="og:type" content="website">
<meta property="og:url" content="{canonical}">
<meta property="og:site_name" content="kiln">
<meta name="twitter:card" content="summary">
<meta name="twitter:title" content="{esc(title)}">
<meta name="twitter:description" content="{esc(description)}">
<script type="application/ld+json">{json.dumps(schema,ensure_ascii=False).replace('<','\\u003c')}</script>
<!-- /generated:seo -->\n'''
    markup = markup.replace('</head>',tags+'</head>')
    path.write_text(markup)
    url = ET.SubElement(sitemap,f'{{{ns}}}url')
    ET.SubElement(url,f'{{{ns}}}loc').text = canonical
ET.indent(sitemap)
ET.ElementTree(sitemap).write(ROOT/'sitemap.xml',encoding='utf-8',xml_declaration=True)
print(f'Rendered canonical/social metadata, structured breadcrumbs and sitemap for {len(paths)} pages.')
