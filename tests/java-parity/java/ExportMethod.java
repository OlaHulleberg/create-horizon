// SPDX-License-Identifier: MIT
// Based on Delete228/create-horizon's development-only AST exporter.
// Create method bodies are read at test time, never embedded in this fixture.
import com.sun.source.tree.*;
import com.sun.source.util.*;
import javax.tools.*;
import java.nio.file.*;
import java.util.*;

public class ExportMethod {
    public static void main(String[] args) throws Exception {
        var compiler = ToolProvider.getSystemJavaCompiler();
        if (compiler == null) throw new IllegalStateException("A JDK, not a JRE, is required");
        try (var files = compiler.getStandardFileManager(null, null, null)) {
            var task = (JavacTask) compiler.getTask(null, files, null,
                List.of("-proc:none"), null, files.getJavaFileObjects(args[0]));
            var tree = task.parse().iterator().next();
            var positions = Trees.instance(task).getSourcePositions();
            var source = Files.readString(Path.of(args[0]));
            var bodies = new ArrayList<String>();
            new TreeScanner<Void, Void>() {
                private String enclosingClass = "";
                public Void visitClass(ClassTree type, Void unused) {
                    var previous = enclosingClass;
                    enclosingClass = type.getSimpleName().toString();
                    try { return super.visitClass(type, unused); }
                    finally { enclosingClass = previous; }
                }
                public Void visitMethod(MethodTree method, Void unused) {
                    boolean selected = args.length < 3
                        || args[2].equals("class:" + enclosingClass)
                        || args[2].equals("arity:" + method.getParameters().size())
                        || (!method.getParameters().isEmpty()
                            && args[2].equals("first:" + method.getParameters().getFirst().getType()));
                    if (selected && method.getName().contentEquals(args[1]) && method.getBody() != null) {
                        var body = method.getBody();
                        bodies.add(source.substring((int) positions.getStartPosition(tree, body),
                            (int) positions.getEndPosition(tree, body)));
                    }
                    return super.visitMethod(method, unused);
                }
            }.scan(tree, null);
            if (bodies.size() != 1)
                throw new IllegalStateException("Expected exactly one method: " + Arrays.toString(args));
            System.out.print(bodies.getFirst());
        }
    }
}
