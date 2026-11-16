// In-process MCP tools through which Claude updates Tandem's UI. Every call is forwarded to
// Tandem, which validates it and answers with the text Claude sees.

import { z } from 'zod';

const SERVER_NAME = 'tandem';
const TOOL_PREFIX = `mcp__${SERVER_NAME}__`;

const TOOLS = [
  {
    name: 'propose_plan',
    description:
      'Show the user the plan for this task: the goal, the "done when" criteria and small numbered steps. Call it again with the full plan to revise it.',
    shape: {
      goal: z.string().min(1).max(500),
      done_when: z.array(z.string().min(1).max(300)).min(1).max(10),
      steps: z
        .array(
          z.object({
            kind: z.enum(['create_file', 'edit_file', 'run_command']),
            target: z.string().min(1).max(500),
            summary: z.string().min(1).max(200),
            language: z.string().min(1).max(40),
          }),
        )
        .min(1)
        .max(40),
    },
  },
  {
    name: 'show_step',
    description: 'Show guidance for the active step at the help level Tandem named. Always say why.',
    shape: {
      step: z.number().int().min(1),
      summary: z.string().min(1).max(2000),
      why: z.string().min(1).max(4000),
      code: z.string().max(20000).optional(),
      pseudocode: z.string().max(10000).optional(),
      command: z.string().max(2000).optional(),
      breakdown: z
        .array(
          z.object({
            part: z.string().min(1).max(300).describe('Copied exactly from the code or command.'),
            explanation: z.string().min(1).max(2000),
          }),
        )
        .max(60)
        .optional(),
      path: z.string().max(500).optional(),
      anchor: z.string().max(500).optional(),
    },
  },
  {
    name: 'report_check',
    description:
      'Report the verdict after checking the user\'s work on a step, or the whole task against its "done when" criteria.',
    shape: {
      scope: z.enum(['step', 'task']),
      step: z.number().int().min(1).optional(),
      verdict: z.enum(['pass', 'needs_changes']),
      explanation: z.string().min(1).max(6000),
      findings: z
        .array(
          z.object({
            path: z.string().max(500).optional(),
            line: z.number().int().min(1).optional(),
            problem: z.string().min(1).max(1000),
            why: z.string().min(1).max(2000),
            hint: z.string().max(2000).optional(),
          }),
        )
        .max(20)
        .optional(),
    },
  },
  {
    name: 'step_done',
    description: 'During a takeover, record that you finished a step.',
    shape: {
      step: z.number().int().min(1),
      summary: z.string().min(1).max(1000),
    },
  },
  {
    name: 'read_editor_buffer',
    description:
      "Read a project file as it is in the user's editor right now, including unsaved changes.",
    shape: {
      path: z.string().min(1).max(500),
    },
  },
];

/** The full names Claude Code gives the Tandem tools. */
export const TANDEM_TOOL_NAMES = TOOLS.map(({ name }) => `${TOOL_PREFIX}${name}`);

/** Builds the `tandem` SDK MCP server whose tools ask Tandem through `askHost`. */
export function createTandemServer({ sdk, askHost }) {
  const tools = TOOLS.map(({ name, description, shape }) =>
    sdk.tool(name, description, shape, async (args) => {
      const answer = await askHost({ request: 'tool', name, input: args });
      return {
        content: [{ type: 'text', text: answer.text ?? '' }],
        isError: answer.ok !== true,
      };
    }),
  );
  return sdk.createSdkMcpServer({ name: SERVER_NAME, version: '1.0.0', tools });
}
