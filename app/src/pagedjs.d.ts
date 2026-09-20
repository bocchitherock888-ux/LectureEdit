declare module 'pagedjs' {
  export class Previewer {
    polisher: { destroy(): void };
    chunker: { destroy(): void };
    preview(
      content: HTMLElement | DocumentFragment | string,
      stylesheets?: Array<string | Record<string, string>>,
      renderTo?: HTMLElement | string,
    ): Promise<{ total: number }>;
  }
}
