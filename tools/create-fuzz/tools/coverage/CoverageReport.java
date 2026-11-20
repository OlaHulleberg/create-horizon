import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.jar.*;
import org.jacoco.core.analysis.*;
import org.jacoco.core.tools.ExecFileLoader;

/** Offline analysis of Create binaries only. JaCoCo remains an external tool. */
public class CoverageReport {
    public static void main(String[] args) throws Exception {
        ExecFileLoader data = new ExecFileLoader();
        File merged = new File(args[1]);
        if (merged.exists()) data.load(merged);
        data.load(new File(args[0]));
        data.save(merged, false);
        CoverageBuilder coverage = new CoverageBuilder();
        Analyzer analyzer = new Analyzer(data.getExecutionDataStore(), coverage);
        String[] includes = args[3].split(":");
        try (JarFile jar = new JarFile(args[2])) {
            var entries = jar.entries();
            while (entries.hasMoreElements()) {
                var entry = entries.nextElement();
                String name = entry.getName();
                if (!name.endsWith(".class")) continue;
                String clazz = name.substring(0, name.length() - 6).replace('/', '.');
                boolean match = Arrays.stream(includes).anyMatch(p -> clazz.matches(p.replace(".", "\\.").replace("*", ".*").replace("?", ".")));
                if (match) {
                    Path dumped = Path.of(args[5], name);
                    Path parent = dumped.getParent();
                    String base = dumped.getFileName().toString().replaceFirst("\\.class$", "");
                    if (Files.isDirectory(parent)) try (var files = Files.list(parent)) {
                        var candidates = files.filter(p -> p.getFileName().toString().startsWith(base + ".")).sorted().toList();
                        for (Path candidate : candidates) {
                            String suffix = candidate.getFileName().toString().substring(base.length() + 1);
                            if (suffix.matches("[0-9a-f]{16}\\.class")) {
                                // Unexecuted loaded classes still use their transformed denominator.
                                if (!Files.exists(dumped)) dumped = candidate;
                                if (data.getExecutionDataStore().get(Long.parseUnsignedLong(suffix.substring(0, 16), 16)) != null) {
                                    dumped = candidate; break;
                                }
                            }
                        }
                    }
                    try (InputStream in = Files.exists(dumped) ? Files.newInputStream(dumped) : jar.getInputStream(entry)) {
                        analyzer.analyzeClass(in, name);
                    }
                }
            }
        }
        if (!coverage.getNoMatchClasses().isEmpty()) throw new IllegalStateException("Create class IDs differ from instrumented classes: " + coverage.getNoMatchClasses());
        List<IClassCoverage> classes = new ArrayList<>(coverage.getClasses());
        classes.sort(Comparator.comparing(IClassCoverage::getName));
        int covered = 0, total = 0, missed = 0;
        StringBuilder report = new StringBuilder("class\tcovered\ttotal\n");
        StringBuilder never = new StringBuilder("class\tmethod\tline\tmissed branches\n");
        for (var c : classes) {
            var b = c.getBranchCounter();
            covered += b.getCoveredCount(); total += b.getTotalCount();
            report.append(c.getName()).append('\t').append(b.getCoveredCount()).append('\t').append(b.getTotalCount()).append('\n');
            for (var m : c.getMethods()) for (int line = m.getFirstLine(); line <= m.getLastLine(); line++) {
                int count = m.getLine(line).getBranchCounter().getMissedCount();
                if (count > 0) { missed += count; never.append(c.getName()).append('\t').append(m.getName()).append(m.getDesc()).append('\t').append(line).append('\t').append(count).append('\n'); }
            }
        }
        Files.writeString(Path.of(args[4], "classes.tsv"), report);
        Files.writeString(Path.of(args[4], "never-reached.tsv"), never);
        Files.writeString(Path.of(args[4], "summary.txt"), covered + "/" + total + " branches covered; " + missed + " never reached\n");
        System.out.println(covered);
    }
}
