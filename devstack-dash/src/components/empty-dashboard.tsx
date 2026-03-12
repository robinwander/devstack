import { motion } from "framer-motion";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Play } from "lucide-react";
import { api, queryKeys } from "@/lib/api";
import { staggerContainer, staggerItem, fadeInUp } from "@/lib/motion";

export function EmptyDashboard({ projects }: { projects: { id: string; name: string; stacks: string[]; path: string; config_exists: boolean }[] }) {
  const queryClient = useQueryClient();

  const upMutation = useMutation({
    mutationFn: ({ stack, project_dir }: { stack: string; project_dir: string }) =>
      api.up({ stack, project_dir, no_wait: true }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs });
      toast.success("Stack starting…");
    },
    onError: (err) => toast.error(err.message),
  });

  const projectsWithStacks = projects.filter((p) => p.config_exists && p.stacks.length > 0);

  return (
    <motion.div
      initial="hidden"
      animate="visible"
      variants={staggerContainer}
      className="flex-1 flex items-center justify-center overflow-y-auto p-6"
    >
      <div className="max-w-md w-full">
        <motion.div variants={fadeInUp} className="mb-8">
          <h2 className="text-xl font-semibold text-ink mb-2">No stacks running</h2>
          <p className="text-sm text-ink-secondary leading-relaxed">
            Start a stack from below, or run{" "}
            <code className="px-1.5 py-0.5 bg-surface-sunken text-ink rounded-sm font-mono text-xs">
              devstack up
            </code>{" "}
            in your project directory.
          </p>
        </motion.div>

        {projectsWithStacks.length > 0 && (
          <motion.div initial="hidden" animate="visible" variants={staggerContainer} className="space-y-2">
            {projectsWithStacks.map((project) =>
              project.stacks.map((stack) => (
                <motion.button
                  key={`${project.id}-${stack}`}
                  variants={staggerItem}
                  onClick={() => upMutation.mutate({ stack, project_dir: project.path })}
                  disabled={upMutation.isPending}
                  className="w-full flex items-start gap-3 px-4 py-3 bg-surface-raised border border-line hover:border-accent/30 rounded-md transition-colors text-left group"
                >
                  <div className="w-8 h-8 bg-accent/10 rounded-md flex items-center justify-center shrink-0 group-hover:bg-accent/15 transition-colors mt-0.5">
                    <Play className="w-3.5 h-3.5 text-accent" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-semibold text-ink">{stack}</div>
                    <div className="text-xs text-ink-tertiary truncate mt-0.5">{project.name} · {project.stacks.length > 1 ? `${project.stacks.length} stacks` : "1 stack"}</div>
                  </div>
                </motion.button>
              ))
            )}
          </motion.div>
        )}
      </div>
    </motion.div>
  );
}
