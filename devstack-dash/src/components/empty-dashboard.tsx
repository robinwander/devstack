import { motion } from "framer-motion";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Play, Terminal } from "lucide-react";
import { api, queryKeys } from "@/lib/api";
import { staggerContainer, staggerItem, fadeInUp } from "@/lib/motion";

export function EmptyDashboard({ projects }: { projects: { id: string; name: string; stacks: string[]; path: string; config_exists: boolean }[] }) {
  const queryClient = useQueryClient();

  const upMutation = useMutation({
    mutationFn: ({ stack, project_dir }: { stack: string; project_dir: string }) =>
      api.up({ stack, project_dir, no_wait: true }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs });
      toast.success("Stack starting...");
    },
    onError: (err) => toast.error(err.message),
  });

  const projectsWithStacks = projects.filter((p) => p.config_exists && p.stacks.length > 0);

  return (
    <motion.div
      initial="hidden"
      animate="visible"
      variants={staggerContainer}
      className="flex-1 flex items-center justify-center overflow-y-auto p-4"
    >
      <div className="text-center max-w-sm">
        <motion.div variants={fadeInUp} className="mb-8">
          {/* Decorative stack illustration */}
          <div className="relative w-20 h-20 mx-auto mb-6">
            <div className="absolute inset-0 bg-primary/5 border border-primary/10" />
            <div className="absolute inset-2 bg-primary/8 border border-primary/15" />
            <div className="absolute inset-4 flex items-center justify-center">
              <Terminal className="w-7 h-7 text-primary/40" />
            </div>
          </div>
          <h2 className="text-xl font-semibold text-foreground/90 mb-2">No active stacks</h2>
          <p className="text-sm text-muted-foreground/65">
            Start a stack from below, or run{" "}
            <code className="px-1.5 py-0.5 bg-secondary text-foreground/70 font-mono text-xs">
              devstack up
            </code>{" "}
            in your project
          </p>
        </motion.div>

        {projectsWithStacks.length > 0 && (
          <motion.div initial="hidden" animate="visible" variants={staggerContainer} className="space-y-1.5">
            {projectsWithStacks.map((project) =>
              project.stacks.map((stack) => (
                <motion.button
                  key={`${project.id}-${stack}`}
                  variants={staggerItem}
                  onClick={() => upMutation.mutate({ stack, project_dir: project.path })}
                  disabled={upMutation.isPending}
                  className="w-full flex items-center gap-3 px-4 py-3 bg-card/50 border border-border hover:border-primary/20 hover:bg-card transition-all text-left group"
                  whileHover={{ x: 2 }}
                  whileTap={{ scale: 0.99 }}
                >
                  <div className="w-8 h-8 bg-primary/10 border border-primary/15 flex items-center justify-center shrink-0 group-hover:bg-primary/15 transition-colors">
                    <Play className="w-3.5 h-3.5 text-primary" />
                  </div>
                  <div className="min-w-0">
                    <div className="text-sm font-medium text-foreground/90">{stack}</div>
                    <div className="text-xs text-muted-foreground/45 truncate">{project.name}</div>
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
