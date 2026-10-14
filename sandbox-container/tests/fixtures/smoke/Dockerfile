FROM busybox AS build
RUN mkdir /out && echo layer-one > /out/keep && echo doomed > /out/gone

FROM busybox
COPY --from=build /out /out
# Deleting a file that lives in a lower layer exercises overlay2 whiteouts.
RUN rm /out/gone
CMD ["ls", "/out"]
