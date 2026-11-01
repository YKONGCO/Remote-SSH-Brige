// The desktop uses the native dialog in Rust. This also supports browser previews.
type SaveHandle = {
  createWritable(): Promise<{
    write(data: Blob): Promise<void>;
    close(): Promise<void>;
    abort(): Promise<void>;
  }>;
};

export async function chooseBrowserSave(
  name: string,
): Promise<((blob: Blob) => Promise<void>) | null> {
  const picker = (
    window as Window & {
      showSaveFilePicker?: (options: {
        suggestedName: string;
      }) => Promise<SaveHandle>;
    }
  ).showSaveFilePicker;
  if (picker) {
    let handle: SaveHandle;
    try {
      handle = await picker.call(window, { suggestedName: name });
    } catch (error) {
      if ((error as Error).name === "AbortError") return null;
      throw error;
    }
    return async (blob) => {
      const writer = await handle.createWritable();
      try {
        await writer.write(blob);
        await writer.close();
      } catch (error) {
        await writer.abort().catch(() => {});
        throw error;
      }
    };
  }
  return async (blob) => {
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = name;
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
}
