package dev.createfuzz;

import java.nio.file.Files;
import java.nio.file.attribute.FileTime;

/** Standalone: javac RequestQueue.java RequestQueueTest.java; java -ea ...RequestQueueTest. */
public class RequestQueueTest {
    public static void main(String[] args) throws Exception {
        var inbox = Files.createTempDirectory("fair-inbox");
        try {
            var old = Files.writeString(inbox.resolve("z.json"), "[]");
            var newer = Files.writeString(inbox.resolve("a.json"), "[]");
            Files.setLastModifiedTime(old, FileTime.fromMillis(1));
            Files.setLastModifiedTime(newer, FileTime.fromMillis(2));
            Files.writeString(inbox.resolve("ignored.tmp"), "[");
            var queue = new RequestQueue<String>();
            queue.discover(inbox);
            queue.discover(inbox);
            assert queue.poll().path().equals(old) : "arrival order beats filename order";
            Files.delete(old);
            queue.resume("sweep");
            assert queue.poll().path().equals(newer) : "small request precedes resumed sweep";
            Files.delete(newer);
            var rpc = Files.writeString(inbox.resolve("fluid-host-x.json"), "{}");
            queue.discover(inbox);
            assert queue.poll().value().equals("sweep") : "new arrivals cannot jump FIFO";
            queue.resume("sweep");
            assert queue.poll().path().equals(rpc) : "RPC waits at most one case quantum";
            assert queue.poll().value().equals("sweep");
            assert queue.poll() == null : "discovery does not duplicate pending requests";
        } finally {
            try (var files = Files.list(inbox)) {
                for (var file : files.toList()) Files.delete(file);
            }
            Files.delete(inbox);
        }
    }
}
