/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AttachmentCard, type AttachmentCardLabels } from "./AttachmentCard";
import { ImageViewerContext } from "./ImageViewerContext";
import type { FileMediaPlayerProps } from "./FileMediaPlayer";
import * as api from "@/lib/api";
import { createT } from "@/i18n";

vi.mock("./FileMediaPlayer", () => ({
  FileMediaPlayer: ({ src, kind, labels }: FileMediaPlayerProps) => (
    <div data-testid="player" data-kind={kind} data-src={src} data-load-error={labels?.loadError} data-loading={labels?.loading} />
  ),
}));

const tr = createT("en");
const labels: AttachmentCardLabels = {
  open: tr("attach.open"), reveal: tr("attach.reveal"), copyPath: tr("attach.copyPath"),
  copyImage: tr("attach.copyImage"), addToComposer: tr("attach.addToComposer"),
  viewImage: tr("image.view"), previewBroken: tr("attach.preview.broken"),
  mediaLoadError: tr("media.loadError"), mediaLoading: tr("media.loading"),
};

let originalLang: string;
beforeEach(() => {
  originalLang = document.documentElement.lang;
  document.documentElement.lang = "en";
  vi.spyOn(api, "isTauri").mockReturnValue(true);
  vi.spyOn(api, "isDesktopHost").mockReturnValue(true);
  vi.spyOn(api, "pathsClassify").mockResolvedValue([]);
  vi.spyOn(api, "mediaImageThumb");
  vi.spyOn(api, "pathOpen").mockResolvedValue(undefined as never);
  vi.spyOn(api, "pathReveal").mockResolvedValue(undefined as never);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.documentElement.lang = originalLang;
});

function expectNoFilesystem() {
  expect(api.pathsClassify).not.toHaveBeenCalled();
  expect(api.mediaImageThumb).not.toHaveBeenCalled();
  expect(api.pathOpen).not.toHaveBeenCalled();
  expect(api.pathReveal).not.toHaveBeenCalled();
}

describe("inline CLI attachments", () => {
  it.each(["card", "chip"] as const)("renders inline image in %s, preserves filename and limits actions", async (variant) => {
    const attachment = { path: "data:image/png;base64,AA==", name: "original filename.png", isDir: false };
    const open = vi.fn();
    render(<ImageViewerContext.Provider value={{ open, close: vi.fn(), isOpen: () => false, copyImage: async () => true }}>
      <AttachmentCard attachment={attachment} labels={labels} variant={variant} onAddToComposer={vi.fn()} />
    </ImageViewerContext.Provider>);
    const image = screen.getByRole("img", { name: attachment.name });
    expect(image.getAttribute("src")).toBe(attachment.path);
    fireEvent.click(image);
    expect(open).toHaveBeenCalledWith([{ src: attachment.path, title: attachment.name }], 0);
    fireEvent.contextMenu(image);
    expect(screen.queryByText(labels.reveal)).toBeNull();
    expect(screen.queryByText(labels.copyPath)).toBeNull();
    expect(screen.queryByText(labels.addToComposer)).toBeNull();
    expect(screen.getByText(labels.copyImage)).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.error(image);
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("button").getAttribute("title")).toBe(labels.previewBroken);
    await Promise.resolve();
    expectNoFilesystem();
  });

  it.each(["audio/wav", "video/mp4"])("opens and closes %s using the shared media player", async (mime) => {
    const attachment = { path: `data:${mime};base64,AA==`, name: "original recording", isDir: false };
    render(<AttachmentCard attachment={attachment} labels={labels} />);
    const toggle = screen.getByRole("button", { name: attachment.name });
    expect(screen.queryByTestId("player")).toBeNull();
    fireEvent.click(toggle);
    const player = await screen.findByTestId("player");
    expect(player.getAttribute("data-src")).toBe(attachment.path);
    expect(player.getAttribute("data-kind")).toBe(mime.split("/")[0]);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(toggle);
    expect(screen.queryByTestId("player")).toBeNull();
    expectNoFilesystem();
  });

  it.each(["audio/wav", "video/mp4"])("provides localized %s player fallbacks and preserves label precedence", async (mime) => {
    document.documentElement.lang = "de";
    const de = createT("de");
    const attachment = { path: `data:${mime};base64,AA==`, name: "recording", isDir: false };
    const minimalLabels = { ...labels, mediaLoadError: undefined, previewBroken: undefined, mediaLoading: undefined };
    const { rerender } = render(<AttachmentCard attachment={attachment} labels={minimalLabels} />);
    fireEvent.click(screen.getByRole("button", { name: attachment.name }));
    const player = await screen.findByTestId("player");
    expect(player.getAttribute("data-load-error")).toBe(de("media.loadError"));
    expect(player.getAttribute("data-loading")).toBe(de("media.loading"));

    rerender(<AttachmentCard attachment={attachment} labels={{ ...minimalLabels, previewBroken: labels.previewBroken, previewPending: tr("attach.preview.pending") }} />);
    expect(player.getAttribute("data-load-error")).toBe(labels.previewBroken);
    expect(player.getAttribute("data-loading")).toBe(tr("attach.preview.pending"));

    rerender(<AttachmentCard attachment={attachment} labels={labels} />);
    expect(player.getAttribute("data-load-error")).toBe(labels.mediaLoadError);
    expect(player.getAttribute("data-loading")).toBe(labels.mediaLoading);
    expectNoFilesystem();
  });

  it.each(["audio/wav", "video/mp4", "image/png", "text/plain"])("keeps %s removable without an optional label", (mime) => {
    document.documentElement.lang = "de";
    const attachment = { path: `data:${mime};base64,AA==`, name: "attachment", isDir: false };
    const onRemove = vi.fn();
    const { rerender } = render(<AttachmentCard attachment={attachment} labels={labels} variant="chip" onRemove={onRemove} />);
    const removeButton = screen.getByRole("button", { name: createT("de")("composer.attachRemove") });
    if (mime.startsWith("audio/") || mime.startsWith("video/")) {
      const mediaCard = removeButton.closest<HTMLElement>(".att-card-media");
      expect(mediaCard?.style.position).toBe("relative");
      expect(mediaCard?.querySelector(".att-card__btn")).toBeTruthy();
    }
    fireEvent.click(removeButton);
    expect(onRemove).toHaveBeenCalledExactlyOnceWith(attachment);

    const remove = tr("composer.attachRemove");
    rerender(<AttachmentCard attachment={attachment} labels={{ ...labels, remove }} variant="chip" onRemove={onRemove} />);
    fireEvent.click(screen.getByRole("button", { name: remove }));
    expect(onRemove).toHaveBeenCalledTimes(2);

    rerender(<AttachmentCard attachment={attachment} labels={{ ...labels, remove }} variant="chip" />);
    expect(screen.queryByRole("button", { name: remove })).toBeNull();
    expectNoFilesystem();
  });

  it("shows unknown inline files without pretending they are filesystem links", () => {
    render(<AttachmentCard attachment={{ path: "data:text/html,<h1>hello</h1>", name: "notes.html", isDir: false }} labels={labels} />);
    const button = screen.getByRole("button", { name: "notes.html" });
    expect(button.hasAttribute("disabled")).toBe(true);
    fireEvent.click(button);
    fireEvent.contextMenu(button);
    expect(screen.queryByText(labels.open)).toBeNull();
    expect(screen.queryByText(labels.reveal)).toBeNull();
    expectNoFilesystem();
  });

  it("keeps filesystem open and reveal actions for local files", () => {
    render(<AttachmentCard attachment={{ path: "/tmp/notes.txt", name: "notes.txt", isDir: false }} labels={labels} />);
    const button = screen.getByRole("button", { name: "notes.txt" });
    fireEvent.click(button);
    expect(api.pathOpen).toHaveBeenCalledWith("/tmp/notes.txt");
    fireEvent.contextMenu(button);
    fireEvent.click(screen.getByText(labels.reveal));
    expect(api.pathReveal).toHaveBeenCalledWith("/tmp/notes.txt");
  });
});
