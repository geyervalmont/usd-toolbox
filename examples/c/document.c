#include "usd_toolbox.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Link to the versioned C library; release result and document handles separately. */
int main(void) {
    const char *source = "#usda 1.0\ndef Xform \"Asset\" {\n"
        "def Mesh \"Triangle\" {\n"
        "uniform token subdivisionScheme = \"none\"\n"
        "point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]\n"
        "int[] faceVertexCounts = [3]\nint[] faceVertexIndices = [0,1,2]\n}\n}\n";
    const char *query = "{\"operation\":\"query\",\"request\":{\"fields\":[\"name\"]}}";
    uint64_t document = 0, result = 0;
    size_t length = 0, report_length = 0;
    assert(usd_toolbox_document_open_v1((const uint8_t *)source, strlen(source), NULL, 0, &document) == USD_TOOLBOX_STATUS_OK);
    assert(usd_toolbox_document_request_v1(document, (const uint8_t *)query, strlen(query), &result) == USD_TOOLBOX_STATUS_OK);
    assert(usd_toolbox_document_release_v1(document) == USD_TOOLBOX_STATUS_OK);
    assert(usd_toolbox_result_copy_v1(result, NULL, 0, &length, NULL, 0, &report_length) == USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    uint8_t *bytes = (uint8_t *)malloc(length + 1);
    uint8_t *report = (uint8_t *)malloc(report_length + 1);
    assert(bytes != NULL && report != NULL);
    assert(usd_toolbox_result_copy_v1(result, bytes, length, &length, report, report_length, &report_length) == USD_TOOLBOX_STATUS_OK);
    bytes[length] = 0;
    assert(strstr((const char *)bytes, "Asset") != NULL);
    assert(strstr((const char *)bytes, "positions") == NULL);
    puts((const char *)bytes);
    free(bytes);
    free(report);
    assert(usd_toolbox_result_release_v1(result) == USD_TOOLBOX_STATUS_OK);
    assert(usd_toolbox_result_release_v1(result) == USD_TOOLBOX_STATUS_INVALID_ARGUMENT);
    return 0;
}
