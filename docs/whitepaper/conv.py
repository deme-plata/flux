#!/usr/bin/env python3
# Minimal, focused markdown -> LaTeX for the Flux whitepaper (DeepSeek narrative + eval.md).
import re, sys

def normalize_unicode(t):
    rep = {
        0x2011: '-', 0x2013: '--', 0x2014: '---',      # hyphen/en/em dash
        0x2018: "'", 0x2019: "'", 0x201C: '``', 0x201D: "''",  # smart quotes
        0x00A0: ' ', 0x2009: ' ', 0x200A: ' ', 0x202F: ' ', 0x2002: ' ', 0x2003: ' ',  # spaces
        0x2026: '...',
    }
    out = []
    for ch in t:
        o = ord(ch)
        if o < 128:
            out.append(ch)
        elif o in rep:
            out.append(rep[o])
        else:
            out.append('?')  # surface any unhandled non-ASCII rather than break pdflatex
    return ''.join(out)

def esc(t):
    t = t.replace('\\', '\x01')
    for c in ['&', '%', '#', '_', '{', '}']:
        t = t.replace(c, '\\' + c)
    t = t.replace('~', '\\textasciitilde{}').replace('^', '\\textasciicircum{}')
    t = t.replace('\x01', '\\textbackslash{}')
    return t

def texttt(s):
    s = s.replace('\\', '\\textbackslash{}')
    for c in ['&', '%', '#', '_', '{', '}']:
        s = s.replace(c, '\\' + c)
    return '\\texttt{' + s + '}'

def inline(t):
    t = normalize_unicode(t)
    codes = []
    t = re.sub(r'`([^`]*)`', lambda m: (codes.append(m.group(1)), '\x00%d\x00' % (len(codes)-1))[1], t)
    t = esc(t)
    t = re.sub(r'\*\*([^*]+)\*\*', lambda m: '\\textbf{' + m.group(1) + '}', t)
    t = re.sub(r'\*([^*]+)\*', lambda m: '\\emph{' + m.group(1) + '}', t)
    t = re.sub('\x00(\\d+)\x00', lambda m: texttt(codes[int(m.group(1))]), t)
    return t

def convert(md):
    out, in_list, title = [], False, None
    for ln in md.split('\n'):
        ln = ln.rstrip()
        if ln.startswith('# ') and title is None:
            title = normalize_unicode(ln[2:].strip()); continue
        if ln.startswith('## '):
            if in_list: out.append('\\end{itemize}'); in_list = False
            out.append('\\section{' + inline(re.sub(r'^\d+\.\s*', '', ln[3:])) + '}'); continue
        if ln.startswith('### '):
            if in_list: out.append('\\end{itemize}'); in_list = False
            out.append('\\subsection{' + inline(re.sub(r'^\d+\.\d+\s*', '', ln[4:])) + '}'); continue
        if ln.strip() == '---':
            if in_list: out.append('\\end{itemize}'); in_list = False
            continue
        m = re.match(r'^\s*\d+\.\s+(.*)', ln) or re.match(r'^\s*[-*]\s+(.*)', ln)
        if m:
            if not in_list: out.append('\\begin{itemize}'); in_list = True
            out.append('\\item ' + inline(m.group(1))); continue
        if ln.strip() == '':
            if in_list: out.append('\\end{itemize}'); in_list = False
            out.append(''); continue
        out.append(inline(ln))
    if in_list: out.append('\\end{itemize}')
    return title or 'Flux', '\n'.join(out)

title, body = convert(open(sys.argv[1], encoding='utf-8').read())
open(sys.argv[2], 'w', encoding='utf-8').write(body)
open(sys.argv[3], 'w', encoding='utf-8').write(title)
print('title:', title)
print('non-ascii leftovers (?):', body.count('?'))
