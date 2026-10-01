/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AttachmentCard, type AttachmentCardLabels } from "./AttachmentCard";
import { ImageViewerContext } from "./ImageViewerContext";
import * as api from "@/lib/api";
import { createT } from "@/i18n";

vi.mock("./FileMediaPlayer", () => ({
  FileMediaPlayer: ({ src, kind }: { src: string; kind: string }) => <div data-testid="player" data-kind={kind} data-src={src} />,
}));

const tr = createT("en");
const labels: AttachmentCardLabels = {
  open: tr("attach.open"), reveal: tr("attach.reveal"), copyPath: tr("attach.copyPath"),
  copyImage: tr("attach.copyImage"), addToComposer: tr("attach.addToComposer"),
  viewImage: tr("image.view"), previewBroken: tr("attach.preview.broken"),
  mediaLoadError: tr("media.loadError"), mediaLoading: tr("media.loading"),
};

beforeEach(() => {
  vi.spyOn(api, "isTauri").mockReturnValue(true);
  vi.spyOn(api, "isDesktopHost").mockReturnValue(true);
  vi.spyOn(api, "pathsClassify").mockResolvedValue([]);
  vi.spyOn(api, "mediaImageThumb");
  vi.spyOn(api, "pathOpen").mockResolvedValue(undefined as never);
  vi.spyOn(api, "pathReveal").mockResolvedValue(undefined as never);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

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
