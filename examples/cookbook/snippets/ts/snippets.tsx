// The TypeScript lines the cookbook pages show. The `docs:begin` / `docs:end` markers delimit what a page
// quotes (site/scripts/build-cookbook.mjs copies it). `../check.sh` compiles this file with `tsc --strict`
// against the generated bindings and the runtime's sources.
import {
  AuthError,
  type Auth,
  type Feed,
  type SignUp,
  SubmitError,
  type Uploads,
  NotesQueryHandle,
  createNote,
  outbox,
} from "@cookbook/core";
import { useSignal } from "@undra/runtime/react";
import type { ReactElement } from "react";

// scaffolding
declare function Login(props: { auth: Auth }): ReactElement;
declare function Home(props: { user: string }): ReactElement;

// docs:begin auth-ts
export function Root({ auth }: { auth: Auth }) {
  const session = useSignal(auth.session);
  return session.kind === "signedOut" ? <Login auth={auth} /> : <Home user={session.user} />;
}

export async function logIn(auth: Auth, email: string, password: string): Promise<string | null> {
  try {
    await auth.signIn(email, password);
    return null;
  } catch (error) {
    return error instanceof AuthError.BadCredentials ? "Wrong email or password" : String(error);
  }
}
// docs:end

// docs:begin paging-ts
export function FeedView({ feed }: { feed: Feed }) {
  const posts = useSignal(feed.visible);
  return (
    <>
      <input onChange={(e) => void feed.setTopic(e.target.value)} placeholder="Search" />
      {posts.map((post) => <p key={post.id}>{post.title}</p>)}
      {/* The core ignores a call while a page is in flight and after the last page. */}
      <button onClick={() => void feed.loadMore()}>More</button>
    </>
  );
}
// docs:end

// docs:begin forms-ts
export function SignUpView({ form }: { form: SignUp }) {
  const errors = useSignal(form.errors);
  const valid = useSignal(form.valid);
  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await form.submit();
        } catch (error) {
          if (error instanceof SubmitError.EmailTaken) alert("That email is already registered");
        }
      }}
    >
      <input type="email" onChange={(e) => void form.setEmail(e.target.value)} onBlur={() => void form.blur("email")} />
      {errors.map((error) => <p key={error.message}>{error.message}</p>)}
      <button disabled={!valid}>Create account</button>
    </form>
  );
}
// docs:end

// docs:begin upload-ts
export function UploadsView({ uploads }: { uploads: Uploads }) {
  const rows = useSignal(uploads.uploads);
  return rows.map((row) => (
    <progress key={row.id} value={row.sent} max={Math.max(row.total, 1)} />
  ));
}
// docs:end

// docs:begin offline-ts
export async function notesScreen() {
  const notes = await NotesQueryHandle.create("inbox");   // cached on disk: shown before the network answers
  void notes.data;

  // Shows at once; returns when the server has it, even if that is after the tunnel.
  await createNote("inbox", "Buy milk", false);

  const waiting = (await outbox()).pending;               // "2 changes waiting to sync"
  return waiting;
}
// docs:end
