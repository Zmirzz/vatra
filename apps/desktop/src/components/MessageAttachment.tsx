import { useEffect, useState } from "react";
import { FileText, Image as ImageIcon, Loader2 } from "lucide-react";
import { readFileBytes, type FileRecord } from "../files/client";
import { useClient } from "../rpc/context";
import { useAppStore } from "../state/store";
import { useFiles } from "../files/store";

export function MessageAttachment({ file }: { file: FileRecord }) {
  const client = useClient();
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let objectUrl: string | undefined;
    if (file.media_type.startsWith("image/")) {
      void readFileBytes(client, file.session_id, file).then(async (bytes) => {
        if (!active) return;
        try {
          const blob = new Blob([bytes], { type: file.media_type });
          objectUrl = URL.createObjectURL(blob);
          setUrl(objectUrl);
        } catch {
          setError("Could not load image.");
        }
      }).catch(() => setError("Could not load image."));
    }
    return () => {
      active = false;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [client, file.id, file.revision, file.session_id]);

  if (file.media_type.startsWith("image/")) {
    if (error) {
      return (
        <div className="message-attachment message-attachment-error" role="alert">
          <ImageIcon size={14} aria-hidden="true" />
          <span>{error}</span>
        </div>
      );
    }
    if (!url) {
      return (
        <div className="message-attachment message-attachment-loading" role="status" aria-label="Loading image">
          <Loader2 size={14} aria-hidden="true" />
        </div>
      );
    }
    return (
      <img
        className="message-attachment-img"
        src={url}
        alt={file.name}
        title={file.name}
        loading="lazy"
        decoding="async"
      />
    );
  }

  return (
    <div className="message-attachment message-attachment-doc" role="button" tabIndex={0}>
      <FileText size={15} aria-hidden="true" />
      <span title={file.name}>{file.name}</span>
    </div>
  );
}

export function MessageAttachments({ seq }: { seq: number }) {
  const session = useAppStore((state) => state.sessions.find((candidate) => candidate.id === state.activeSessionId));
  if (!session) return null;
  const records = useFiles((s) => s.records[session.id]);
  const links = useFiles((s) => s.links[session.id]);
  const shown = (records ?? []).filter((file) => (links ?? []).some(([message, id]) => message === seq && id === file.id));
  if (!shown.length) return null;
  return (
    <div className="message-attachments" aria-label="Attached files">
      {shown.map((file) => (
        <MessageAttachment key={file.id} file={file} />
      ))}
    </div>
  );
}
