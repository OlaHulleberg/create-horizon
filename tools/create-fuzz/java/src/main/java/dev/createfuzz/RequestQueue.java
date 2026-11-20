package dev.createfuzz;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;

/** FIFO arrivals, with unfinished batches requeued after each disposed case. */
final class RequestQueue<T> {
    record Request<T>(Path path, T value) {}
    private record Arrival(Path path, FileTime time) {}
    private final ArrayDeque<Request<T>> queue = new ArrayDeque<>();
    private final HashSet<Path> waiting = new HashSet<>();

    void discover(Path inbox) {
        var arrivals = new ArrayList<Arrival>();
        try (var files = Files.list(inbox)) {
            files.filter(p -> p.toString().endsWith(".json") && !waiting.contains(p))
                    .forEach(p -> {
                        try { arrivals.add(new Arrival(p, Files.getLastModifiedTime(p))); }
                        catch (IOException ignored) { /* A withdrawn request is not an arrival. */ }
                    });
        } catch (IOException ignored) { return; }
        arrivals.sort(Comparator.comparing(Arrival::time).thenComparing(a -> a.path().toString()));
        for (var arrival : arrivals) {
            waiting.add(arrival.path());
            queue.addLast(new Request<>(arrival.path(), null));
        }
    }

    void resume(T value) { queue.addLast(new Request<>(null, value)); }

    Request<T> poll() {
        var request = queue.pollFirst();
        if (request != null && request.path() != null) waiting.remove(request.path());
        return request;
    }
}
