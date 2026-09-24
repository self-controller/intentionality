import { memo, type ComponentProps } from "react";
import ReactMarkdown from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";

// The one place markdown becomes elements. Everything rendered through here
// is untrusted -- the write-up is model output until the user edits it -- and
// react-markdown produces React elements, never raw HTML: there is no
// rehype-raw here and there must not be, or a <script> in a summary would run
// inside the app.
//
// remark-math + rehype-katex: `$...$` and `$$...$$`, the syntax the prompt
// asks the model for. rehype-highlight: fenced blocks get hljs-* classes,
// coloured by app.css rather than a shipped theme.
const remarkPlugins = [remarkGfm, remarkMath];
const rehypePlugins = [rehypeKatex, rehypeHighlight];

// Links render but do not navigate. The webview has no opener plugin
// (capabilities grant only `core:`), so a real click would replace the whole
// app with the page and leave no way back. The URL rides in the tooltip.
function Link({ href, children }: ComponentProps<"a">) {
  return (
    <a href={href} title={href} onClick={(e) => e.preventDefault()}>
      {children}
    </a>
  );
}

const components = { a: Link };

// Memoised: its one prop is a string, and a full parse + KaTeX + highlight
// pass is too much to repeat whenever a parent re-renders for another reason.
export default memo(function Markdown({ source }: { source: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={rehypePlugins}
        components={components}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
});
