import { readPhoto } from "@app/fieldbook-core";
import { useEffect, useState } from "react";

/** A photo of a note: the core reads the bytes from its `Fs` port, the page draws them. */
export function Photo({ path }: { readonly path: string }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let revoke: string | null = null;
    let cancelled = false;
    void readPhoto(path).then((bytes) => {
      if (cancelled) return;
      revoke = URL.createObjectURL(new Blob([bytes as BlobPart]));
      setUrl(revoke);
    });
    return () => {
      cancelled = true;
      if (revoke !== null) URL.revokeObjectURL(revoke);
    };
  }, [path]);
  return url === null ? <span className="thumb" /> : <img className="thumb" src={url} alt="A photo attached to the note" />;
}
