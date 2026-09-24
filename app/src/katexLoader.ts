import { useEffect, useState } from 'react';

type Katex = typeof import('katex').default;

let loaded: Katex | null = null;
let loading: Promise<Katex> | null = null;

/** KaTeX and its fonts are only fetched once a formula is actually shown. */
export function loadKatex() {
  loading ??= Promise.all([import('katex'), import('katex/dist/katex.min.css')]).then(([module]) => (loaded = module.default));
  return loading;
}

export function useKatex() {
  const [katex, setKatex] = useState<Katex | null>(loaded);
  useEffect(() => {
    if (katex) return;
    let live = true;
    void loadKatex().then((module) => { if (live) setKatex(() => module); });
    return () => { live = false; };
  }, [katex]);
  return katex;
}
