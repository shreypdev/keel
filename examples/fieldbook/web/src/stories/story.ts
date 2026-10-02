import type { ReactElement } from "react";

/**
 * The shape of a story, Component Story Format 3: a module's default export is the meta (`title`), each named export a story with a
 * `render`. Storybook loads such modules as they are, and `stories.html` renders them with Vite alone, so the sample needs no
 * Storybook dependency to show its screens in their states.
 */
export interface Meta {
  readonly title: string;
}

/** One story: a state of a screen. `render` is a component body (it may use hooks). */
export interface Story {
  readonly name?: string;
  readonly render: () => ReactElement;
}
