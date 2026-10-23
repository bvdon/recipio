import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, errorMessage, queryKeys } from "../api";
import { ErrorBanner } from "./common";

/** The user's name, shown to friends in shared files. */
export default function MyName() {
  const queryClient = useQueryClient();
  const name = useQuery({ queryKey: queryKeys.myName, queryFn: api.getMyName });
  const [draft, setDraft] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (value: string) => api.setMyName(value),
    onSuccess: (value) => {
      queryClient.setQueryData(queryKeys.myName, value);
      setDraft(null);
    },
  });
  if (name.data === undefined) return null;
  const value = draft ?? name.data;
  return (
    <form
      className={name.data ? "my-name" : "my-name unset"}
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate(value);
      }}
    >
      <label>
        Your name <span className="muted">(friends see it on what you send)</span>
        <input value={value} onChange={(e) => setDraft(e.target.value)} placeholder="e.g. Rob" />
      </label>
      {draft !== null && draft !== name.data && (
        <button type="submit" className="button primary" disabled={save.isPending}>
          Save
        </button>
      )}
      {save.isError && <ErrorBanner message={errorMessage(save.error)} />}
    </form>
  );
}
